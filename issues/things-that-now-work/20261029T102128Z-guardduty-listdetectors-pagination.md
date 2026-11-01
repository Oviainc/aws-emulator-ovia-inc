Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 4a0740c4b10bc61036cc0b642f74c6c28951c1b9

# The shared page-token reader honours offset spellings the token minter never writes, so a token this service never issued is served as a page

_Filed: 2026-10-29T10:21:28Z_
_Operation: ListDetectors | Class: pagination_

> _Note: this issue was identified against the code base at commit `4a0740c4b10bc61036cc0b642f74c6c28951c1b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated report in this crate goes through one pair of helpers, `crate::state::paginate` / `crate::state::paginate_window`. Their own documentation states the contract: the token is "the number of entries already returned, as a decimal string", and `Err(token)` — which every caller maps to a 400 naming `nextToken` — "means the token was not one this service issued: either it is not a decimal offset at all, or it points past the end of the list".

The only place a token is MINTED is `Some(end.to_string())` (state.rs:5607), which for a `usize` always writes a plain, unsigned, unpadded decimal (`"0"`, `"1"`, `"50"`). The place a token is READ is `t.parse::<usize>()` (state.rs:5590). Rust's integer `FromStr` additionally accepts an optional leading `+` and any number of leading zeros, so `"+0"`, `"+7"`, `"007"` and `"0000000000000050"` all parse — spellings `end.to_string()` can never produce. Such a token is therefore accepted and served as a page position instead of being refused, which is exactly the case the helper's doc comment says must land in `Err`.

This is one defect in one shared reader, but it reaches every paginated operation in the crate: ListDetectors, ListFindings, ListFilters, ListIPSets, ListThreatIntelSets, ListThreatEntitySets, ListTrustedEntitySets, ListMembers, ListInvitations, ListPublishingDestinations, ListMalwareProtectionPlans, ListMalwareScans, DescribeMalwareScans, ListInvestigations, ListCoverage, ListOrganizationAdminAccounts, ListCustomDetectionRules, ListCustomDetectionRuleAssociations, ListCustomDetectionRuleOrgConfigurations, DescribeOrganizationConfiguration and GetUsageStatistics. For ListInvestigations the modelled `NextToken` pattern `^[a-zA-Z0-9+/=_\-]+$` explicitly admits `+`, so a `"+0"` token passes the request boundary and reaches the lax parse.

## Evidence

The contract the helper states for itself — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:5557-5563:
```
/// Take one page out of an ordered key list.
///
/// `token` is the number of entries already returned, as a decimal string — the
/// only thing a caller may do with it is hand it back. `Err(token)` means the token
/// was not one this service issued: either it is not a decimal offset at all, or it
/// points past the end of the list, which this service never hands out (it answers
/// the last page with no `NextToken`).
```
and restated for the window form — src/state.rs:5579-5581: "Both go through this one function, so the token a report issues and the token it accepts cannot drift between them. `Err(token)` is the token rejection [`paginate`] documents."

The ONLY minting site — src/state.rs:5605-5610:
```
    let end = start.saturating_add(limit).min(len);
    let next = if end < len {
        Some(end.to_string())
    } else {
        None
    };
```

The lax reading site — src/state.rs:5588-5597:
```
    let start = match token.filter(|t| !t.is_empty()) {
        Some(t) => {
            let start = t.parse::<usize>().map_err(|_| t.to_string())?;
            // A token this service issued always names an entry that exists: it is
            // emitted only while `end < len`.
            if start >= len {
                return Err(t.to_string());
            }
            start
        }
```
`t.parse::<usize>()` succeeds for `"+7"` and `"007"`; `end.to_string()` never writes either.

A caller of the shared reader, which maps `Err` to the modelled 400 — src/ops/list_detectors.rs:158-168:
```
        let (page, next_token) = crate::state::paginate(
            &ids,
            input.max_results,
            input.next_token.as_deref(),
            ...
        )
        .map_err(|_| {
            ... crate::state::invalid_parameter_message(
                crate::state::NEXT_TOKEN_MEMBER,
            )
```

The model admits the `+` spelling on the wire for one of the affected listings — `smithy_query(operation="ListInvestigations")`: `"NextToken": {"target": "NextToken", "jsonName": "nextToken", "length": {"min": 1, "max": 2048}, "pattern": "^[a-zA-Z0-9+/=_\\-]+$"}`.

Test coverage: tests/all/client_rejections.rs:363-408 pins the refusal over the token forms `["abc", "-1", "1.5", "99999999999999999999999999", "9999", "1e3"]` against ListDetectors and ListFindings; tests/all/custom_detection_rules.rs:743-763, tests/all/trusted_entity_set_listing.rs:722-737, tests/all/filter_listing.rs:534-548 and tests/all/organization_admin_accounts.rs:594-596 all use `"not-a-token"`, `"9999"` and an out-of-range decimal. No test anywhere in tests/all sends `"+0"` or a zero-padded decimal, so nothing pins the current acceptance.

## Steps to Reproduce

1. Create a detector in us-east-1 (`POST /detector`) so `ListDetectors` has at least one entry.
2. Call `GET /detector?maxResults=1` and observe the answer carries `nextToken: "1"` only when a second detector exists; with one detector the single page carries no `nextToken` at all — so this service has issued no token for this listing.
3. Call `GET /detector?nextToken=%2B0` (the token `+0`). Expected, per src/state.rs:5560-5563: HTTP 400 `InvalidInputException` naming `nextToken`, because `+0` is not a token this service ever issued. Actual: HTTP 200 with the first page, because `"+0".parse::<usize>()` at src/state.rs:5590 yields 0.
4. Call `GET /detector?nextToken=000` — same expected refusal, same actual 200.
5. With two or more detectors, call `GET /detector?maxResults=1&nextToken=001`: the service answers the page at offset 1 although it only ever mints `"1"`, so two distinct token strings name the same page and the token the helper documents as opaque is observably not.
6. Contrast with `GET /detector?nextToken=-1` or `?nextToken=1e3`, which are refused at src/state.rs:5590 as the doc requires (pinned by tests/all/client_rejections.rs:370-377).
