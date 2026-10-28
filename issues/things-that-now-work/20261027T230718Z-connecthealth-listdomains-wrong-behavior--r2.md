Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# ListDomains enumerates every domain a Region holds, ignoring the owning account the model's doc sentence scopes the listing to

_Filed: 2026-10-27T23:07:18Z_
_Operation: ListDomains | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy model documents ListDomains as "Lists Domains for a given account." Every `Domain` record this crate stores carries the account that owns it (`Domain::account_id`, written at create from `crate::state::owning_account_id()`), and the crate already states the account scope once, in `ComAmazonawsConnecthealthConnecthealthState::domains_held_by`, which `CreateDomain` measures its per-account cap through.

The `ListDomains` handler does not use that scope at all: it enumerates `store.domains_in(&region)` — documented as "Every `Domain` record `region` holds, whatever its status" — and then filters only by `status` and by the pagination cursor. So the listing's scope is the whole Region's inventory rather than the calling account's, which is wider than the one the model's sentence names, and it is the one lookup in this crate that drops a scope key the record carries and a sibling path enforces.

No behavioral impact today: `owning_account_id()` takes no request context, so every record in this emulator carries the same account and the wider scope currently returns the same set. The defect is that the account predicate is simply absent from the one listing whose documentation names it, so the moment the owning account becomes request-derived (as the Region already is, through `region_of(ctx)`) the listing reports domains of accounts that did not ask for them.

## Evidence

Model (smithy_query query="doc" shape="ListDomains"):
  {"shape": "ListDomains", "documentation": "<p>Lists Domains for a given account.</p>"}

Handler — no account predicate (services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_domains.rs:178-181):
```
        let mut matching = store
            .domains_in(&region)
            .filter(|domain| wanted.is_none_or(|status| domain.status == status))
            .filter(|domain| page.starts_after(&domain.domain_id));
```
The reader it walks is Region-scoped only (services/.../src/state.rs:811-824):
```
    /// Every `Domain` record `region` holds, whatever its status, in `domainId` order.
    pub fn domains_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a Domain> + 'a {
```
The account scope the crate DOES state, and which `CreateDomain` uses (services/.../src/state.rs:776-785):
```
    /// How many `Domain` records `account_id` currently holds IN `region` ...
    pub fn domains_held_by(&self, region: &str, account_id: &str) -> usize {
        self.domains_in(region)
            .filter(|domain| domain.account_id == account_id)
            .count()
    }
```
and the record field it reads (services/.../src/state.rs:309-312):
```
    /// The account that OWNS this domain — the account the create was signed for.
    /// Stored on the record, so every response projects the domain's own account
    /// rather than recomputing one per read.
    pub account_id: String,
```
`CreateDomain` resolves and uses it (services/.../src/ops/create_domain.rs:316): `let held = store.domains_held_by(&region, &account_id);`

Test coverage: tests/all/domain_response_projection.rs:151 (`domain_responses_project_the_owning_account_and_the_onboarding_generation`) asserts each response's `accountId` equals the account segment of the domain's ARN; no test asserts that ListDomains restricts the listing to that account.

## Steps to Reproduce

1. Read services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_domains.rs:178-181 and note the only predicates are `status` and the page cursor.
2. Read services/.../src/state.rs:811-824 (`domains_in`) and confirm it filters on `domain.region` alone.
3. Read services/.../src/state.rs:776-785 (`domains_held_by`) and services/.../src/ops/create_domain.rs:316 and confirm the account scope exists in this crate and is applied on the create path.
4. Run `smithy_query(query="doc", shape="ListDomains")` and compare the sentence "Lists Domains for a given account." with the Region-wide enumeration the handler performs.
