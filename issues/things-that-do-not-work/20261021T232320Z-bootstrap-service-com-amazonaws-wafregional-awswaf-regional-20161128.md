Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the WAF Regional emulator

_Filed: 2026-10-21T23:23:20Z_
_Operation: service-com-amazonaws-wafregional-awswaf-regional-20161128 | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 81 operations in `services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, holding zero state. Nothing works end-to-end.

Implement a FIRST minimal, stateful slice: the IPSet lifecycle of AWS WAF Classic (regional), which is the service's primary resource and exercises the change-token protocol every mutating WAF Classic call requires. Six operations, in this order:

1. **GetChangeToken** — input: none. Output: `ChangeToken` (string 1..128). Must mint a NEW unique token per call, record it in state with status `PROVISIONED`, and return it. No constant string.
2. **CreateIPSet** — input: `Name` (required), `ChangeToken` (required). Output: `IPSet` {`IPSetId` (newly generated ResourceId), `Name`, `IPSetDescriptors`: []} and the `ChangeToken` that was consumed. Must persist the new IPSet under its generated id so later reads see it; the token must be marked consumed/`INSYNC`.
3. **UpdateIPSet** — input: `IPSetId` (required), `ChangeToken` (required), `Updates` (list<IPSetUpdate>, len 1.., required). Each `IPSetUpdate` = {`Action`: INSERT|DELETE, `IPSetDescriptor`: {`Type`: IPV4|IPV6, `Value`: CIDR string 1..50}}. Output: `ChangeToken`. Must APPLY the updates to the stored IPSet: INSERT adds the descriptor (idempotently), DELETE removes it. Unknown `IPSetId` → `WAFNonexistentItemException` (HTTP 400).
4. **GetIPSet** — input: `IPSetId` (required). Output: `IPSet` read back from state, including every descriptor inserted by step 3 and none that were deleted. Unknown id → `WAFNonexistentItemException` (HTTP 400).
5. **ListIPSets** — input: `NextMarker` (optional), `Limit` (0..100, default 0). Output: `IPSets` (list<IPSetSummary> = {`IPSetId`, `Name`}) built from the stored map, plus `NextMarker` when more remain. Must honour `Limit` (0 = all/service default) and resume from `NextMarker`.
6. **DeleteIPSet** — input: `IPSetId` (required), `ChangeToken` (required). Output: `ChangeToken`. Must remove the IPSet from state; a non-empty IPSet (still has descriptors) → `WAFNonEmptyEntityException` (HTTP 400); unknown id → `WAFNonexistentItemException` (HTTP 400). After deletion, `GetIPSet` must return `WAFNonexistentItemException` and `ListIPSets` must not include it.

NEVER STUBS: each of the six must compute its response from persisted state in the shared engine store (`crate::state::service_state(&*self.engine)`). No canned IDs without storage, no echoing the input back, no hollow 200s. Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: AWSWAF_Regional_20161128.<Operation>`.

## State Model

In `ComAmazonawsWafregionalAwswafRegional20161128State`:

- `ip_sets: BTreeMap<String /* IPSetId */, IPSetEntry>` where `IPSetEntry { ip_set_id: String, name: String, descriptors: Vec<IPSetDescriptorEntry> }`
- `IPSetDescriptorEntry { kind: IPSetDescriptorType /* IPV4|IPV6 */, value: String /* CIDR */ }` — ordered as inserted, deduplicated on (kind, value)
- `change_tokens: BTreeMap<String /* token */, ChangeTokenStatus>` — a token starts `PROVISIONED` when minted by GetChangeToken and becomes `INSYNC` once a mutating call consumes it (this also backs a later `GetChangeTokenStatus`)
- `ip_set_name_index: BTreeMap<String /* Name */, String /* IPSetId */>` (optional, for name-collision / `WAFDisallowedNameException` checks)
- a monotonic counter or UUID source used to generate `IPSetId` and change-token values so each create returns a distinct, stable id

All of it lives in the ONE shared engine so a create in one request is visible to the next request.

Everything outside these six operations (WebACL/Rule/RuleGroup CRUD, the other match sets, AssociateWebACL, logging/permission policy, tagging, GetSampledRequests) stays 501 and is filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-wafregional-awswaf-regional-20161128/OPERATIONS.md` lists all 81 operations and their contracts: GetChangeToken (line 207, no input, output `ChangeToken`), CreateIPSet (line 45, input `Name`+`ChangeToken`, output `IPSet`+`ChangeToken`, errors incl. WAFDisallowedNameException/WAFStaleDataException), UpdateIPSet (line 453, input `IPSetId`+`ChangeToken`+`Updates` list<IPSetUpdate> len 1.., errors incl. WAFNonexistentItemException), GetIPSet (line 225, input `IPSetId`, output `IPSet`, error WAFNonexistentItemException), ListIPSets (line 333, input `NextMarker`+`Limit` 0..100, output `IPSets` list<IPSetSummary>+`NextMarker`), DeleteIPSet (line 123, input `IPSetId`+`ChangeToken`, errors WAFNonEmptyEntityException/WAFNonexistentItemException/WAFReferencedItemException). Shape detail from the Smithy model: `IPSet{IPSetId required, Name, IPSetDescriptors required}`, `IPSetUpdate{Action required (ChangeAction), IPSetDescriptor required}`, `IPSetDescriptor{Type required (IPV4|IPV6), Value required (CIDR, len 1..50)}`, `IPSetSummary{IPSetId required, Name required}`. All-stub census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-wafregional-awswaf-regional-20161128/src/ops/` matches every handler file, e.g. `src/ops/create_ip_set.rs`, `src/ops/get_ip_set.rs`, `src/ops/update_ip_set.rs`, `src/ops/list_ip_sets.rs`, `src/ops/delete_ip_set.rs`, `src/ops/get_change_token.rs`. No tests assert any of this behaviour (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `waf-regional` client at it.
2. Call `GetChangeToken` — expect a fresh non-empty ChangeToken (and a different value on a second call); currently HTTP 501 NotImplemented.
3. Call `CreateIPSet` with Name="bootstrap-ipset" and that ChangeToken — expect an IPSet with a generated IPSetId, Name echoed from storage, and an empty IPSetDescriptors list.
4. Call `GetChangeToken` again, then `UpdateIPSet` with that IPSetId, the new token, and Updates=[{Action: INSERT, IPSetDescriptor: {Type: IPV4, Value: "192.0.2.0/24"}}].
5. Call `GetIPSet` with the IPSetId — expect IPSetDescriptors to contain exactly {IPV4, 192.0.2.0/24}, proving the update was persisted.
6. Call `ListIPSets` with Limit=100 — expect an IPSetSummary with the same IPSetId and Name.
7. Call `UpdateIPSet` with a new token and Updates=[{Action: DELETE, IPSetDescriptor: {Type: IPV4, Value: "192.0.2.0/24"}}], then `DeleteIPSet` with a new token — expect success now that the set is empty.
8. Call `GetIPSet` again — expect WAFNonexistentItemException (HTTP 400), and `ListIPSets` must no longer list it.
9. Call `GetIPSet` with a never-created id — expect WAFNonexistentItemException (HTTP 400), not 501 and not a hollow 200.
