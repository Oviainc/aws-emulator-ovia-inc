Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# No path makes the emulated account the payer, so every payer-gated operation answers 403 forever (Organizations CreateOrganization not observed)

_Filed: 2026-10-24T04:59:11Z_
_Operation: CreateBillingGroup | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The payer-account authorization gate in `src/service.rs` denies a request whenever `crate::state::caller_is_payer_account()` is false, and that function compares the single presented caller account (`engine::emulator_account_id()`) against `state.payer_account_id`. Nothing in the SERVICE ever assigns `payer_account_id`: the only writers in the whole crate are the integration tests. In a running emulator the field is therefore `None` for the life of the process, `is_payer_account()` is false for every request, and ALL 31 payer-gated operations (every implemented create / associate / list / update / delete, plus every still-scaffolded one) answer HTTP 403 `AccessDeniedException` "Account <id> is not authorized to call <Op>. Only payer account is authorized".

Expected: real Billing Conductor serves the control plane to the organization's payer (management) account — the normal case for anyone who uses the service. This emulator already models the act that makes the presented account a management account: the Organizations emulator's `CreateOrganization` sets `master_account_id = engine::emulator_account_id()` and stores that account as the management account. After a client creates an organization (or through any other explicit designation seam), `POST /create-billing-group`, `/create-pricing-plan`, `/list-billing-groups`, ... must be SERVED (200) rather than denied, so a realistic workflow (create organization -> create pricing plan -> create billing group -> associate accounts -> list) can run end to end over HTTP.

Actual: the implemented control plane is unreachable over HTTP. No sequence of HTTP requests can produce a single 200 from any gated operation, so every implemented handler is dead code from a client's point of view; the only operations a client can reach are the three ungated tagging operations, which then 404 because no resource can ever be created.

IMPORTANT constraint for a fix: the gate itself must stay. The recorded corpus shows the emulated account id being DENIED when nothing has designated it as a payer (`billingconductor-payer-account-access-denied`, and the 7 behavioral traces of the payer-gate issue), so the default empty store must keep answering 403. What is missing is the propagation: the payer account must be recorded when the organization state says the emulated account is the management account (i.e. observe the Organizations emulator's organization/management account), leaving a store with no organization denying exactly as it does today.

## Evidence

Spec/grounding: AWS Billing Conductor is a payer-account-only API (the already-fixed issue `20261022T200837Z-billingconductor-payer-account-authorization-gate-absent-across-the.md` establishes the 403 for a non-payer caller and the message construction), which implies the converse — the payer account IS authorized and is served.

Handler/state: `src/service.rs:457` `if self.payer_gated && !crate::state::caller_is_payer_account(&*self.engine) { return payer_account_denial(...) }` with `PAYER_UNGATED_OPERATIONS = [ListTagsForResource, TagResource, UntagResource]` (src/service.rs:167); `src/state.rs:819` `is_payer_account()` compares against `payer_account_id`; `src/state.rs:562-575` documents the field as `None` by default and states outright that "A test (or a future Organizations surface) designates a payer account by setting it, which is the only way the control plane becomes reachable". `grep -r payer_account_id` finds assignments ONLY under `tests/all/*` (11 test files) — no production code path sets it.

Cross-service fact the emulator already holds: `services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/create_organization.rs:252` sets `master_account_id = ::engine::emulator_account_id()` and inserts that account as the management account, so the emulator does know when the presented account is a payer — billingconductor just never looks (its Cargo.toml has no dependency and `state.rs` has no reader).

Test coverage: every test that exercises a gated operation first writes `payer_account_id = Some(engine::emulator_account_id())` directly into the store (e.g. `tests/all/billing_group_core_loop.rs:40`), so no test covers the gate's behaviour through the surface a client actually has; `tests/all/payer_account_gate.rs` only pins the denial side.

## Steps to Reproduce

1. Boot the emulator from this checkout with the default (empty) in-memory engine
2. POST /create-pricing-plan with body {"Name":"Plan1"} -> HTTP 403 AccessDeniedException "Account <id> is not authorized to call CreatePricingPlan. Only payer account is authorized"
3. Create an organization through the Organizations emulator (POST Organizations CreateOrganization), which records the emulated account as the organization's management/master account
4. Repeat POST /create-pricing-plan, then POST /create-billing-group and POST /list-billing-groups -> still HTTP 403 for all of them, so no billing group, pricing plan, pricing rule or custom line item can ever be created over HTTP
5. Confirm the store's view: nothing but a test ever assigns `payer_account_id`, so `caller_is_payer_account()` can never return true at runtime
