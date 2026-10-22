Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the wafv2 emulator

_Filed: 2026-10-21T21:18:42Z_
_Operation: service-com-amazonaws-wafv2-awswaf-20190729 | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 59 wafv2 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no WAFv2 client can do anything at all. Implement a FIRST, SMALL slice: the stateful lifecycle of the primary resource, the WebACL, plus the IPSet it typically references.

Wire protocol (from README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: AWSWAF_20190729.<OperationName>`, sigv4 signing name `wafv2`.

## Happy path to implement (in order)

1. **CreateIPSet** — in: `Name` (required, `^[\w\-]+$`), `Scope` (required, CLOUDFRONT|REGIONAL), `IPAddressVersion` (required, IPV4|IPV6), `Addresses` (required), `Description`, `Tags`. Out: `Summary` (IPSetSummary: `Name`, `Id`, `Description`, `LockToken`, `ARN`). Must mint a fresh UUID `Id` (model pattern `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`), a UUID `LockToken`, and an ARN of the modeled form `arn:aws:wafv2:<region>:<account>:<regional|global>/ipset/<Name>/<Id>`, and PERSIST the whole set. A second CreateIPSet with the same (Scope, Name) must fail `WAFDuplicateItemException` (HTTP 400).
2. **CreateWebACL** — in: `Name` (required), `Scope` (required), `DefaultAction` (required), `VisibilityConfig` (required), `Rules`, `Description`, `CustomResponseBodies`, `Tags`. Out: `Summary` (WebACLSummary: `Name`, `Id`, `Description`, `LockToken`, `ARN`). Persist the full WebACL including `Rules` (an IPSetReferenceStatement pointing at the ARN from step 1 must round-trip byte-for-byte). Duplicate (Scope, Name) ⇒ `WAFDuplicateItemException`.
3. **GetWebACL** — in: `Name`, `Scope`, `Id` (or `ARN`). Out: `WebACL` (required members `Name`, `Id`, `ARN`, `DefaultAction`, `VisibilityConfig`, plus `Rules`, `Description`, `Capacity`, `LabelNamespace`, `ManagedByFirewallManager`) and `LockToken`. Must return exactly what CreateWebACL stored, with the CURRENT `LockToken`. Unknown Name/Id ⇒ `WAFNonexistentItemException` (HTTP 400), not 200 and not 501.
4. **ListWebACLs** — in: `Scope` (required), `Limit`, `NextMarker`. Out: `WebACLs` (list<WebACLSummary>), `NextMarker`. Must enumerate only the WebACLs stored under the requested `Scope`, honour `Limit` and return/accept `NextMarker` for the next page (omit `NextMarker` on the last page).
5. **UpdateWebACL** — in: `Name`, `Scope`, `Id`, `DefaultAction`, `VisibilityConfig`, `LockToken` (all required), plus `Rules`, `Description`. Out: `NextLockToken`. Must verify `LockToken` matches the stored one — mismatch ⇒ `WAFOptimisticLockException` (HTTP 400) and NO mutation — then overwrite the stored WebACL, rotate the lock token and return the new one as `NextLockToken`. A following GetWebACL must show the updated rules/description.
6. **DeleteWebACL** — in: `Name`, `Scope`, `Id`, `LockToken` (all required). Out: no members. Stale `LockToken` ⇒ `WAFOptimisticLockException`; unknown item ⇒ `WAFNonexistentItemException`. After success, GetWebACL ⇒ `WAFNonexistentItemException` and ListWebACLs no longer shows it.

Also implement **GetIPSet** / **DeleteIPSet** symmetrically (GetIPSet out: `IPSet` + `LockToken`) so the IPSet created in step 1 can be read back and cleaned up; DeleteIPSet while a live WebACL references its ARN must fail `WAFAssociatedItemException` (HTTP 400).

NEVER STUBS: every one of these must compute its response from the persisted state in `crate::state::service_state(&*self.engine)`. Hardcoded summaries, echoed inputs, random IDs that are not stored, and constant lock tokens are failures — a Create followed by a Get must return what was created, and the lock-token checks above are only meaningful against stored tokens.

## State Model

In `src/state.rs` (`ComAmazonawsWafv2Awswaf20190729State`), keyed per Scope so CLOUDFRONT and REGIONAL namespaces are independent:

- `web_acls: HashMap<(Scope, String /*Id*/), WebAclEntry>` where `WebAclEntry { name, id, arn, scope, default_action, description, rules: Vec<Rule>, visibility_config, capacity: i64, custom_response_bodies, captcha_config, challenge_config, token_domains, label_namespace: String, lock_token: String, tags: Vec<Tag>, created_at }`
- `web_acl_names: HashMap<(Scope, String /*Name*/), String /*Id*/>` — duplicate-name detection and Name+Id lookup for Get/Update/Delete.
- `ip_sets: HashMap<(Scope, String /*Id*/), IpSetEntry>` where `IpSetEntry { name, id, arn, scope, description, ip_address_version, addresses: Vec<String>, lock_token: String, tags: Vec<Tag> }`, plus `ip_set_names: HashMap<(Scope, String), String>`.
- `arn_index: HashMap<String /*ARN*/, ResourceRef>` so GetWebACL-by-ARN and the IPSet in-use check (scan `web_acls[*].rules` for IPSetReferenceStatement ARNs) resolve from state.
- Lock tokens are stored per entry and rotated (new UUIDv4) on every successful mutation; the previous value must stop working.

All other wafv2 operations (managed rule groups, logging configurations, rule groups, regex pattern sets, API keys, statistics, AssociateWebACL/GetWebACLForResource, tagging, ...) stay scaffolded 501 for now and are tracked as separate gap issues.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/` matches all 59 handler files, including `create_web_acl.rs`, `get_web_acl.rs`, `list_web_ac_ls.rs`, `update_web_acl.rs`, `delete_web_acl.rs`, `create_ip_set.rs`, `get_ip_set.rs`, `delete_ip_set.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501. Field names, requiredness, patterns and error lists above are taken from the crate's generated `OPERATIONS.md` (sections CreateIPSet, GetIPSet, DeleteIPSet, CreateWebACL, GetWebACL, ListWebACLs, UpdateWebACL, DeleteWebACL) and from the Smithy model shapes `WebACL`/`WebACLSummary` (smithy_query), whose `LockToken` doc states: "WAF returns a token to your get and list requests... WAF uses the token to ensure that no changes have been made to the entity since you last retrieved it. If a change has been made, the update fails with a WAFOptimisticLockException." The protocol facts (awsJson1_1, `X-Amz-Target: AWSWAF_20190729.<Op>`) come from `OPERATIONS.md` "Wire protocol" and `README.md`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI wafv2 client at it (`aws wafv2 ... --scope REGIONAL --endpoint-url <emulator>`).
2. CreateIPSet Name=boot-ips Scope=REGIONAL IPAddressVersion=IPV4 Addresses=["10.0.0.0/8"] — expect 200 with Summary.Id (UUID), Summary.ARN, Summary.LockToken. Today: HTTP 501 NotImplemented.
3. CreateWebACL Name=boot-acl Scope=REGIONAL DefaultAction={Allow:{}} VisibilityConfig={SampledRequestsEnabled:true,CloudWatchMetricsEnabled:true,MetricName:boot} Rules=[one rule with an IPSetReferenceStatement using the ARN from step 2] — expect 200 with Summary.{Id,ARN,LockToken}.
4. GetWebACL Name=boot-acl Scope=REGIONAL Id=<Id> — expect the stored WebACL back (same Name/Id/ARN/DefaultAction/Rules) plus LockToken.
5. ListWebACLs Scope=REGIONAL Limit=1 — expect boot-acl in WebACLs; create a second ACL and verify Limit/NextMarker paginate instead of returning everything.
6. UpdateWebACL with the LockToken from step 4, changed Description and Rules — expect NextLockToken != old token; retry the SAME call with the old token and expect WAFOptimisticLockException (HTTP 400). GetWebACL must now show the new Description/Rules.
7. DeleteWebACL Name=boot-acl Scope=REGIONAL Id=<Id> LockToken=<NextLockToken> — expect 200; then GetWebACL ⇒ WAFNonexistentItemException and ListWebACLs no longer lists it.
8. DeleteIPSet for boot-ips BEFORE deleting the ACL ⇒ expect WAFAssociatedItemException; after the ACL is deleted ⇒ expect 200, and GetIPSet afterwards ⇒ WAFNonexistentItemException.
