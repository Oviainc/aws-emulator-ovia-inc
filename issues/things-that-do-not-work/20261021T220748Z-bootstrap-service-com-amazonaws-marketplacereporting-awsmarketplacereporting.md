Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Marketplace Reporting emulator

_Filed: 2026-10-21T22:07:48Z_
_Operation: service-com-amazonaws-marketplacereporting-awsmarketplacereporting | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole service is a fresh scaffold: `src/ops/get_buyer_dashboard.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetBuyerDashboard"))` (HTTP 501), and `src/state.rs` has `// TODO: add service-specific state fields`. AWS Marketplace Reporting models exactly ONE operation, so the bootstrap happy path is that single operation, implemented statefully.

## Happy path (1 operation — the whole public surface)

1. **GetBuyerDashboard** — `POST /getBuyerDashboard` (restJson1), success 200.
   - Input: `dashboardIdentifier` (required, len 1..1023, must match the modeled ARN pattern `^arn:aws:aws-marketplace::[0-9]{12}:AWSMarketplace/ReportingData/(Agreement_V1/Dashboard/AgreementSummary_V1|BillingEven…`), `embeddingDomains` (required, list<string>, len 1..2).
   - Output: `embedUrl` (required), `dashboardIdentifier` (required), `embeddingDomains` (required).
   - Behaviour to implement:
     * Mint a NEW embed URL per call, recorded in service state — not a constant string. The URL must be distinct per invocation (it carries a one-shot bearer token) and must be recoverable from state by its token so redemption semantics can be modeled.
     * Echo back `dashboardIdentifier` and `embeddingDomains` from the STORED session record that was just persisted, not straight from the request struct, so a later read of state and the response agree.
     * Record the AWS-documented lifetimes on the session: the embed URL token is valid 5 minutes from generation and is single-use once redeemed; the resulting session lifetime is 1 hour and runs independently of the 5-minute window. A second redemption of the same token, or a redemption after the 5-minute window, must not be treated as valid.
     * Register the requested `embeddingDomains` as the allow list for that session (including the `*` subdomain wildcard form, e.g. `https://*.amazon.com`).
   - Errors from the model, wired to real conditions rather than never returned:
     * `BadRequestException` (400) for a malformed/unsupported `dashboardIdentifier` that passes the regex but names a dashboard this emulator does not know, and for an `embeddingDomains` entry that is not a fully qualified domain.
     * `AccessDeniedException` (403) / `UnauthorizedException` (401) for a caller that is not an Organizations management account or a delegated administrator registered for procurement insights — representable as a per-account flag in state so tests can drive both the allowed and denied branch.
     * `InternalServerException` (500) only for genuine internal failure (e.g. `service_state` returning `None`).

No canned, hardcoded or purely echoed response is acceptable: the handler must compute its output from a record it persists first, and the generated `embedUrl` must be looked up afterwards in state.

## State Model

Add to `ComAmazonawsMarketplacereportingAwsmarketplacereportingState`:

- `dashboards: HashMap<String /* dashboardIdentifier ARN */, Dashboard>` — seeded/lazily created entries for the ARNs the model's pattern admits. `Dashboard { identifier: String, account_id: String, kind: DashboardKind /* AgreementSummary_V1, BillingEvent…, parsed from the ARN tail */ }`.
- `embed_sessions: HashMap<String /* opaque token in the embedUrl */, EmbedSession>` where `EmbedSession { token: String, dashboard_identifier: String, embedding_domains: Vec<String>, embed_url: String, created_at: SystemTime, token_expires_at: SystemTime /* created_at + 5 min */, session_expires_at: SystemTime /* created_at + 1 h */, redeemed_at: Option<SystemTime> }`.
- `access: HashMap<String /* caller account id */, CallerAccess>` with `CallerAccess { is_org_management_account: bool, procurement_insights_registered: bool, delegated_admin: bool }`, used to decide 200 vs 401/403.
- A monotonic `next_token_seq: u64` (or equivalent) so each `embed_url` is unique and reproducible in tests.

## Done when

A client can POST a valid `dashboardIdentifier` + one or two `embeddingDomains`, get 200 with a fresh `embedUrl` and the same identifier/domains echoed from state; two successive calls return two DIFFERENT `embedUrl`s; an unregistered caller gets the modeled 401/403 and an unknown-but-pattern-valid dashboard gets 400. Nothing in this service returns 501 anymore after this slice, since the service has only this one operation.

## Evidence

`services/service-com-amazonaws-marketplacereporting-awsmarketplacereporting/OPERATIONS.md`: 1 operation, `GetBuyerDashboard`, `POST /getBuyerDashboard -> 200`, restJson1, sigv4 name `aws-marketplace`; input `dashboardIdentifier` (required, len 1..1023, ARN pattern) + `embeddingDomains` (required, list, len 1..2); output `embedUrl`, `dashboardIdentifier`, `embeddingDomains` all required; errors AccessDeniedException(403), BadRequestException(400), InternalServerException(500), UnauthorizedException(401). Handler `src/ops/get_buyer_dashboard.rs:4` carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 179 returns `Scaffolded::not_implemented("GetBuyerDashboard")`. `src/state.rs:8` is an empty struct with `// TODO: add service-specific state fields`. The 5-minute single-use bearer token and 1-hour session lifetime are documented verbatim in the operation's modeled docs (reproduced at `src/ops/get_buyer_dashboard.rs:52-59`), as is the management-account / procurement-insights delegated-admin access restriction and the `*` subdomain wildcard form of `embeddingDomains`. No test asserts any of this: `tests/all/state_roundtrip.rs` is the scaffold's state round-trip only.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /getBuyerDashboard with {"dashboardIdentifier": "arn:aws:aws-marketplace::123456789012:AWSMarketplace/ReportingData/Agreement_V1/Dashboard/AgreementSummary_V1", "embeddingDomains": ["https://*.amazon.com"]} — currently HTTP 501 NotImplemented; expected HTTP 200 with a non-empty `embedUrl`, and `dashboardIdentifier`/`embeddingDomains` matching the request.
3. Repeat the same POST — expected a DIFFERENT `embedUrl` (each URL carries a fresh single-use 5-minute token); a repeated/constant value proves a canned response.
4. POST with two domains ["https://a.example.com", "https://b.example.com"] — expected 200 with both returned; POST with three domains — expected 400/ValidationException from the modeled len 1..2 constraint.
5. POST with a `dashboardIdentifier` that does not match the modeled ARN pattern — expected the modeled constraint failure, not 501.
6. POST as a caller not registered for procurement insights — expected the modeled UnauthorizedException (401) / AccessDeniedException (403), not 200 and not 501.
