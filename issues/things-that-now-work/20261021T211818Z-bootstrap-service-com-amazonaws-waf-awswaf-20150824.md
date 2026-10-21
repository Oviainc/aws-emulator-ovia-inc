Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the WAF (AWS WAF Classic) emulator

_Filed: 2026-10-21T21:18:18Z_
_Operation: service-com-amazonaws-waf-awswaf-20150824 | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 77 operations in `services/service-com-amazonaws-waf-awswaf-20150824/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. The service therefore has no working emulator at all.

Implement a FIRST, SMALL slice: the IPSet lifecycle, which is AWS WAF Classic's primary resource and the one every client script touches. The wire protocol is `awsJson1_1`: `POST /` with `X-Amz-Target: AWSWAF_20150824.<OperationName>`.

Happy path, in order (fields from OPERATIONS.md):

1. **GetChangeToken** — Input: none. Output: `ChangeToken` (string 1..128). Must mint a NEW unique token per call, record it in state with status `PROVISIONED`, and return it. Every mutating WAF Classic call requires a change token obtained here.
2. **CreateIPSet** — Input: `Name` (required, 1..128), `ChangeToken` (required). Output: `IPSet` { `IPSetId` (required, newly generated unique id), `Name`, `IPSetDescriptors` (required, empty list on create) } plus the `ChangeToken` that was used. Must persist the IPSet keyed by the generated `IPSetId`; the token transitions to `INSYNC` once consumed.
3. **UpdateIPSet** — Input: `IPSetId` (required), `ChangeToken` (required), `Updates` (list<IPSetUpdate>, len 1.., required). Each `IPSetUpdate` is { `Action`: INSERT|DELETE, `IPSetDescriptor`: { `Type`: IPV4|IPV6, `Value`: CIDR string 1..50 } }. Output: `ChangeToken`. Must apply INSERT/DELETE to the stored `IPSetDescriptors` of that IPSet (INSERT adds, DELETE removes a matching Type+Value); unknown `IPSetId` → `WAFNonexistentItemException` (HTTP 400).
4. **GetIPSet** — Input: `IPSetId` (required). Output: `IPSet` read back from state, including every descriptor inserted in step 3 and no descriptor deleted there. Unknown id → `WAFNonexistentItemException` (HTTP 400).
5. **ListIPSets** — Input: `NextMarker` (optional, 1..1224), `Limit` (integer 0..100, default 0). Output: `IPSets` (list<IPSetSummary> { `IPSetId`, `Name` }) built from state for every live IPSet, plus `NextMarker` when the page is truncated. `Limit` must actually bound the page and a returned `NextMarker` must resume after the last item.
6. **DeleteIPSet** — Input: `IPSetId` (required), `ChangeToken` (required). Output: `ChangeToken`. Must remove the IPSet from state; a subsequent GetIPSet must then fail with `WAFNonexistentItemException` and ListIPSets must no longer include it. Deleting an IPSet that still contains descriptors must fail with `WAFNonEmptyEntityException` (HTTP 400), matching WAF Classic's documented rule that an entity must be emptied before deletion; unknown id → `WAFNonexistentItemException`.

No canned, echoed, or hardcoded responses: each response must be computed from persisted state. In particular CreateIPSet must store the IPSet it returns (generating an id, not echoing the name), GetIPSet must read it back, ListIPSets must enumerate the live map, and DeleteIPSet must actually remove it. A generated id that is not stored is a failure.

## State Model

In `src/state.rs` (`ComAmazonawsWafAwswaf20150824State`, reached via `crate::state::service_state(&*self.engine)`):

- `ip_sets: HashMap<String /* IPSetId */, IpSetEntry>` where
  - `IpSetEntry { ip_set_id: String, name: String, descriptors: Vec<IpSetDescriptorEntry> }`
  - `IpSetDescriptorEntry { kind: String /* "IPV4" | "IPV6" */, value: String /* CIDR */ }`
  - insertion order preserved (or a `Vec<String>` of ids / `IndexMap`) so ListIPSets paginates deterministically.
- `change_tokens: HashMap<String /* token */, ChangeTokenEntry>` where `ChangeTokenEntry { token: String, status: String /* PROVISIONED | PENDING | INSYNC */ }`, so GetChangeTokenStatus can later be implemented off the same structure and mutating ops can validate/consume the token.
- `next_id: u64` (or UUID generator) used to mint `IPSetId` and change-token values that are unique across calls.

All of the above lives in the single shared engine-held state so the sequence above is observable across requests.

Out of scope for this first slice (they stay 501 and are filed as individual gap issues later): the ByteMatch/GeoMatch/RegexMatch/RegexPattern/SizeConstraint/SqlInjection/XssMatch set families, Rule / RateBasedRule / RuleGroup / WebACL operations, logging & permission policy, tagging, GetSampledRequests, and CreateWebACLMigrationStack.

## Evidence

`services/service-com-amazonaws-waf-awswaf-20150824/OPERATIONS.md` lists 77 operations for SDK id `WAF`, protocol `awsJson1_1` (`X-Amz-Target: AWSWAF_20150824.<Op>`), and gives the exact contracts used above: GetChangeToken (no input → `ChangeToken`), CreateIPSet (`Name`, `ChangeToken` required → `IPSet`, `ChangeToken`), UpdateIPSet (`IPSetId`, `ChangeToken`, `Updates` list<IPSetUpdate> len 1.. required → `ChangeToken`; errors include `WAFNonexistentItemException` 400), GetIPSet (`IPSetId` → `IPSet`; `WAFNonexistentItemException` 400), ListIPSets (`NextMarker`, `Limit` 0..100 → `IPSets` list<IPSetSummary>, `NextMarker`), DeleteIPSet (`IPSetId`, `ChangeToken` → `ChangeToken`; errors `WAFNonEmptyEntityException`, `WAFNonexistentItemException`, `WAFReferencedItemException` 400). Shape details confirmed via smithy_query: `IPSet{IPSetId required, Name, IPSetDescriptors required}`, `IPSetUpdate{Action: ChangeAction required, IPSetDescriptor required}`, `IPSetDescriptor{Type: IPV4|IPV6 required, Value 1..50 required}`, `IPSetSummary{IPSetId required, Name required}`. All-stub status confirmed: `src/ops/create_ip_set.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`, and the crate README documents that every generated stub starts in that state.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `waf` client at it (region us-east-1, endpoint = emulator).
2. Call GetChangeToken → expect HTTP 200 with a non-empty `ChangeToken`; currently HTTP 501 NotImplemented.
3. Call CreateIPSet with Name="bootstrap-ipset" and that ChangeToken → expect 200 with `IPSet.IPSetId` set, `IPSet.Name` == "bootstrap-ipset", `IPSet.IPSetDescriptors` == [].
4. Call GetChangeToken again, then UpdateIPSet with that IPSetId and Updates=[{Action: INSERT, IPSetDescriptor: {Type: IPV4, Value: "192.0.2.0/24"}}] → expect 200.
5. Call GetIPSet with the IPSetId → expect the IPSet to contain exactly the descriptor IPV4 192.0.2.0/24 (proves the update was persisted, not echoed).
6. Call ListIPSets with Limit=10 → expect the IPSets list to contain an IPSetSummary with that IPSetId and Name.
7. Call DeleteIPSet with a fresh ChangeToken while the descriptor is still present → expect WAFNonEmptyEntityException (HTTP 400).
8. Call UpdateIPSet with Action=DELETE for the same descriptor, then DeleteIPSet → expect 200.
9. Call GetIPSet with the deleted IPSetId → expect WAFNonexistentItemException (HTTP 400), and ListIPSets no longer lists it.
