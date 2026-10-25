Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Connection$billingTier is never assigned, so it is absent from every connection response

_Filed: 2026-10-24T13:51:55Z_
_Operation: GetConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `billingTier` (BillingTier, @range 0..8) is documented on both Connection and ConnectionSummary as "The billing tier this connection is currently assigned" - i.e. the service assigns a tier to a connection, so a connection read back from the real service carries the member.

Actual: CreateConnection hardcodes `billing_tier: None` for every connection it allocates, and no other handler ever sets it (`summary_of` just copies the stored `None`). The member is therefore omitted from every CreateConnection, GetConnection, UpdateConnection, DeleteConnection and ListConnections response body, for every bandwidth and every environment. The response is not computed from state - it is a constant absence - so a client that reads or displays `connection.billingTier` (or a diff against a recorded response) sees a missing member where AWS sends an integer.

## Evidence

Spec: smithy_query doc for com.amazonaws.interconnect#Connection$billingTier and ConnectionSummary$billingTier: "The billing tier this connection is currently assigned", target BillingTier with @range(min:0,max:8). Handler: src/ops/create_connection.rs lines 320-326 sets `billing_tier: None` with a comment that the assignment rule is unknown; src/state.rs `summary_of` line 405 propagates that `None` into ConnectionSummary. No other code assigns it (grep billing_tier).
Tests: tests/all/connection_lifecycle.rs (create_connection_computes_every_server_owned_member_and_persists_it) asserts the other server-owned members (id, arn, sharedId, activationKey, ownerAccount, provider, location, type, state) but makes no assertion about billingTier, so nothing covers it.
Not covered by existing issues: no open or fixed interconnect issue mentions billingTier (checked the four open interconnect issues plus the keyword listing).

## Steps to Reproduce

1. Boot the emulator, pick a seeded environment via ListEnvironments and CreateConnection on it.
2. Inspect the CreateConnection response body: `connection.billingTier` is absent.
3. Call GetConnection(identifier=<new id>) and ListConnections() -> the member is absent from the connection and from the ConnectionSummary as well, for every connection and every bandwidth.
4. Expect an integer in 0..8 derived from the connection's bandwidth/product tier to be present and stable across reads.
