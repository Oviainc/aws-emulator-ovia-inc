Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the com-amazonaws-uxc-awsaccountuxsetting emulator

_Filed: 2026-10-21T21:53:03Z_
_Operation: service-com-amazonaws-uxc-awsaccountuxsetting | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The entire service is a fresh scaffold: all three handlers under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501 `NotImplemented`, and `ComAmazonawsUxcAwsaccountuxsettingState` holds no fields. Implement a first functional slice covering the service's whole (small) surface so a realistic client loop works end-to-end.

### Happy path (implement in this order)

1. **UpdateAccountCustomizations** — `PATCH /v1/account-customizations`, `@idempotent`. Input: `accountColor` (AccountColor enum: darkBlue | green | lightBlue | none | orange | pink | purple | red | teal | yellow), `visibleServices` (list<string>, max len 500), `visibleRegions` (list<string>, max len 100). Must PERSIST the supplied customization into the per-account state and return the resulting stored customization (`accountColor`, `visibleServices`, `visibleRegions`) — read back out of state after the write, not echoed from the input struct. Being `@idempotent`, applying the same request twice must leave identical state and return an identical body. An unknown `accountColor` value or a list exceeding its modeled length must produce `ValidationException` (HTTP 400), not a 200.

2. **GetAccountCustomizations** — `GET /v1/account-customizations`, `@readonly`, no input members. Must return exactly what the last successful UpdateAccountCustomizations stored for the calling account: `accountColor`, `visibleServices`, `visibleRegions`. Before any update has been performed it must return the empty/default customization (no `accountColor` set, or `none`, with empty/absent lists) — never a hardcoded sample. It must not write state.

3. **ListServices** — `GET /v1/services`, `@readonly`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=services)`. Input: `maxResults` (integer, range 1..1000, `@httpQuery`), `nextToken` (string, len 32..512, pattern `^[-A-Za-z0-9+/_]+=*$`, `@httpQuery`). Must return a stable, deterministically ordered catalog of service names from state, truncated to `maxResults`, with `nextToken` set only when more items remain and absent on the final page. A supplied `nextToken` must resume exactly after the last returned item; a token that violates the modeled length/pattern, or a `maxResults` outside 1..1000, must yield `ValidationException` (HTTP 400). Paging through with `maxResults=1` must enumerate every service exactly once with no duplicates or omissions.

### Rules

- NEVER STUBS: every one of the three operations must compute its response from persisted state. Hardcoded bodies, echoing the input back without storing it, and empty 200s are all failures of this issue.
- State must survive across requests via the single shared engine, reached through `crate::state::service_state(&*self.engine)` — never a freshly constructed engine inside a handler.
- Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the three handler files as it is implemented.
- Modeled errors `AccessDeniedException` (403), `InternalServerException` (500), `ThrottlingException` (429) must at least be reachable shapes; `ValidationException` (400) must actually be returned for the constraint violations above.
- `restJson1` wire protocol, sigv4 signing name `uxc`.

## State Model

Add to `ComAmazonawsUxcAwsaccountuxsettingState` (currently `// TODO: add service-specific state fields`):

- `customizations: HashMap<String /* account id from the request context */, AccountUxCustomization>`
  - `AccountUxCustomization { account_color: Option<AccountColor>, visible_services: Vec<String>, visible_regions: Vec<String> }` — the single record written by UpdateAccountCustomizations and read by GetAccountCustomizations. Missing key = default/empty customization.
- `service_catalog: Vec<String>` — the ordered list of known service names backing ListServices (seeded at state construction with a fixed, deterministic set of AWS service identifiers). Ordering must be stable so pagination is coherent.
- `page_tokens: HashMap<String /* opaque nextToken */, usize /* offset into service_catalog */>` — or an equivalent encoded-offset scheme — so ListServices tokens are issued by the emulator, validated on input, and resolve to a real resume position rather than being ignored.

## Evidence

`services/service-com-amazonaws-uxc-awsaccountuxsetting/OPERATIONS.md` lists exactly 3 operations (GetAccountCustomizations, ListServices, UpdateAccountCustomizations) with their routes, members, constraints, traits (@readonly, @idempotent, @paginated) and errors (AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400), and records protocol `restJson1` / SDK id `uxc`. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all three handler files (get_account_customizations.rs, list_services.rs, update_account_customizations.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. `src/state.rs` shows `ComAmazonawsUxcAwsaccountuxsettingState` with only a `// TODO: add service-specific state fields` comment, so no state exists to serve any read.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `uxc` at it.
2. Call GetAccountCustomizations (GET /v1/account-customizations) on a fresh account — expect 200 with an empty/default customization; actual: HTTP 501 NotImplemented.
3. Call UpdateAccountCustomizations (PATCH /v1/account-customizations) with accountColor=teal, visibleServices=["s3","lambda"], visibleRegions=["us-east-1"] — expect 200 echoing the stored record; actual: HTTP 501.
4. Call GetAccountCustomizations again — expect accountColor=teal and the exact lists just stored to be returned from persisted state.
5. Repeat the identical UpdateAccountCustomizations call (it is @idempotent) — expect identical state and identical response body.
6. Call UpdateAccountCustomizations with an invalid accountColor value — expect ValidationException (HTTP 400).
7. Call ListServices (GET /v1/services) with maxResults=1 and follow nextToken until it is absent — expect every catalog service returned exactly once in stable order; actual: HTTP 501.
8. Call ListServices with maxResults=0 or a nextToken that violates the modeled pattern — expect ValidationException (HTTP 400).
