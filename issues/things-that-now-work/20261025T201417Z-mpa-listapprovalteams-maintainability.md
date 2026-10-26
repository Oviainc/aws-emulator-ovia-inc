Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c7879b242a52aaa0fabb95009fbb05c28e2cce49

# The seq-token pagination rule is implemented three times verbatim (and the MaxResults clamp six times), so the copies can drift

_Filed: 2026-10-25T20:14:17Z_
_Operation: ListApprovalTeams | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c7879b242a52aaa0fabb95009fbb05c28e2cce49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three list handlers in this crate implement the same pagination computation line for line: parse `NextToken` as a bare `u64` position (refusing a parse failure with the same ValidationException sentence), clamp `MaxResults` into `1..=MAX_LIST_PAGE`, filter the creation/initiation-ordered records to `record.seq > after`, slice `remaining[..remaining.len().min(page_size)]`, and derive `next_token` as `page.last().filter(|_| remaining.len() > page.len()).map(|r| r.seq.to_string())`. The identical `MaxResults` clamp expression, with the identical comment, additionally appears in three more handlers (six copies total). Because each copy is independent, a fix or a change to one — the token grammar, the "is this a token we issued" rule, the slicing, the next-token condition — silently leaves the others behind; the crate already shows this drift, since the two catalog lists validate that the token resolves to a stored record while these three accept any numeric value. One private helper in this crate (taking the ordered records, the token and MaxResults and answering the page plus the next token) would leave every response byte-identical.

## Evidence

Copy 1 — src/ops/list_approval_teams.rs:169-196: `let after = match input.next_token.as_deref().filter(|t| !t.is_empty()) { None => 0u64, Some(token) => token.parse::<u64>().map_err(|_| ... ValidationException { message: "NextToken is not a token this service issued." ... })? };` / `let page_size = usize::try_from(input.max_results.clamp(1, crate::state::MAX_LIST_PAGE)).unwrap_or(1);` / `let page = &remaining[..remaining.len().min(page_size)];` / `let next_token = page.last().filter(|_| remaining.len() > page.len()).map(|team| team.seq.to_string());`

Copy 2 — src/ops/list_identity_sources.rs:176-203: the same four steps with `source.seq` (`.filter(|source| source.seq > after)`, `.map(|source| source.seq.to_string())`).

Copy 3 — src/ops/list_sessions.rs:226-264: the same four steps with `session.seq` (`.filter(|session| session.seq > after)`, `.map(|session| session.seq.to_string())`).

The clamp alone, with the same comment "`MaxResults` is modelled `range(1..20)` with `default: 20`, so the boundary validator has already closed it; the clamp only keeps the slice width sane.", is also duplicated at src/ops/list_resource_policies.rs:205-208, src/ops/list_policies.rs:175-178 and src/ops/list_policy_versions.rs:192-195.

Existing tests that pin these responses and must stay green: tests/all/approval_team_lifecycle.rs:803 `list_pages_over_every_team_exactly_once`, tests/all/identity_sources.rs:379 `an_empty_store_lists_no_identity_source` and :483 `a_foreign_next_token_is_refused`, tests/all/session_listing.rs:231 `a_team_s_sessions_are_listed_in_initiation_order_and_read_off_the_stored_record` and :810 `a_next_token_this_service_did_not_issue_is_refused`, tests/all/resource_arn_reads.rs:353 `resource_policies_are_paged_out_of_the_resource_record`.

## Steps to Reproduce

1. Open src/ops/list_approval_teams.rs:169-196, src/ops/list_identity_sources.rs:176-203 and src/ops/list_sessions.rs:226-264 side by side and compare the four pagination steps: they differ only in the record type and the field name.
2. Grep the crate for `input.max_results.clamp(1, crate::state::MAX_LIST_PAGE)` and count six occurrences across six handlers.
3. Note the drift the duplication already allowed: src/ops/list_policies.rs:184-193 and src/ops/list_policy_versions.rs:210-223 additionally refuse a token the store does not hold, while the three seq-token copies accept any numeric value.
4. Confirm the fix is behaviour-preserving: factoring the shared steps into one private helper used by the three seq-token lists changes no response member and leaves the tests named above untouched.
