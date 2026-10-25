Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# The MaxResults page-size resolution is implemented eleven times, once per paginated handler

_Filed: 2026-10-25T04:49:15Z_
_Operation: ListPolicies | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation declares its own private `const MAX_PAGE_SIZE: usize = 100;` with the same doc sentence, and then repeats the same two-line resolution expression (`map_or(MAX_PAGE_SIZE, ...).clamp(1, MAX_PAGE_SIZE)` for an optional `MaxResults`, `usize::try_from(input.max_results).unwrap_or(0).clamp(1, MAX_PAGE_SIZE)` for a required one). Eleven copies of one rule can drift: changing the bound or the clamp in one handler silently leaves the other ten serving a different page size, which is exactly the disagreement `crate::state` says it exists to prevent — it already hosts the other shared pagination derivation (the token codec), but not the page-size rule. Hoisting the resolution into `crate::state` beside `encode_key_token`/`decode_key_token` leaves every response and every existing pagination test unchanged.

## Evidence

The crate states the principle it does not follow here — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:7-10:
```
//! store; the helpers below are the bits of derivation several handlers share
//! (the request's Region, the policy ARN, the `PolicySummary` projection and the
//! pagination token codec), kept in one place so two handlers cannot disagree.
```

Eleven copies of the constant (grep `const MAX_PAGE_SIZE: usize`): list_policies.rs:141, list_member_accounts.rs:125, list_resource_sets.rs:135, list_apps_lists.rs:149, list_protocols_lists.rs:145, list_compliance_status.rs:134, list_discovered_resources.rs:149, list_admin_accounts_for_organization.rs:130, list_admins_managing_account.rs:120, list_third_party_firewall_firewall_policies.rs:165, get_protection_status.rs:152 — each with the same doc, e.g. list_policies.rs:137-141 "The largest page `ListPolicies` serves, which is the maximum the model's `com.amazonaws.fms#PaginationMaxResults` declares for `MaxResults` (range 1..100)." and list_member_accounts.rs:121-125 the identical sentence with the operation name swapped.

The computation itself is byte-identical across handlers — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_policies.rs:227-230:
```
        let page_size = input
            .max_results
            .map_or(MAX_PAGE_SIZE, |max| usize::try_from(max).unwrap_or(0))
            .clamp(1, MAX_PAGE_SIZE);
```
and services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_member_accounts.rs:207-210:
```
        let page_size = input
            .max_results
            .map_or(MAX_PAGE_SIZE, |max| usize::try_from(max).unwrap_or(0))
            .clamp(1, MAX_PAGE_SIZE);
```
(the same four lines again at list_compliance_status.rs:228-231, list_resource_sets.rs:227-230, list_discovered_resources.rs:256-259, list_admin_accounts_for_organization.rs:228-231, list_admins_managing_account.rs:219-222, get_protection_status.rs:288-291), with the required-member variant repeated at list_apps_lists.rs:232-234, list_protocols_lists.rs:228-230 and list_third_party_firewall_firewall_policies.rs:286-288:
```
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(0)
            .clamp(1, MAX_PAGE_SIZE);
```

Test coverage that must stay green and pins the behaviour: tests/all/policy_lifecycle.rs, tests/all/apps_lists_enumeration.rs, tests/all/protocols_lists_enumeration.rs, tests/all/resource_set_listing.rs, tests/all/member_accounts.rs, tests/all/compliance_status.rs, tests/all/protection_status.rs, tests/all/admin_accounts_for_organization.rs, tests/all/discovered_resources.rs, tests/all/third_party_firewall_firewall_policies.rs.

## Steps to Reproduce

1. grep -n 'const MAX_PAGE_SIZE: usize' services/service-com-amazonaws-fms-awsfms-20180101/src/ops/*.rs — eleven identical declarations
2. grep -n -A3 'let page_size = input' services/service-com-amazonaws-fms-awsfms-20180101/src/ops/*.rs — eight byte-identical copies of the optional-MaxResults resolution
3. grep -n -A2 'let page_size = usize::try_from' services/service-com-amazonaws-fms-awsfms-20180101/src/ops/*.rs — three byte-identical copies of the required-MaxResults resolution
4. Read services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:7-10, which says the derivations several handlers share are kept in one place so two handlers cannot disagree, and note that the page-size rule is not among them
