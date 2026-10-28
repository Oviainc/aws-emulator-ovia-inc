Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# The catalog page-cut rule is written out twice, in ListPolicies and ListPolicyVersions, instead of once beside page_after_seq

_Filed: 2026-10-27T04:33:25Z_
_Operation: ListPolicies | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two catalog listings each hand-roll the identical page-cut computation: read at most `page_size + 1` entries from a `BTreeMap` (from the start, or `Bound::Excluded(token)..` when a token was supplied), set `more_remain = window.len() > page_size`, truncate to `page_size`, and emit a `NextToken` built from the last reported entry exactly while more remain. Only two things differ between the copies: the map's key type and how the last entry's key is rendered into the token.

This crate already owns that rule once for its RECORD listings — `crate::state::page_after_seq` performs exactly the same cut (filter past the resume position, `more_remain`, truncate, token from the last record iff more remain) and `ListApprovalTeams` / `ListIdentitySources` / `ListSessions` all go through it. The two catalog listings are two more copies of that one rule, so the `more_remain` condition, the truncation width and the "token present exactly while entries remain" invariant can drift between the policy listings and both the record listings and each other.

The fix is a single shared cut helper (generic over the map and over the token renderer) called by both handlers. Every response stays byte-identical and both existing paging tests keep passing unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_policies.rs:196-210:
  `let mut window: Vec<&crate::state::CatalogPolicy> = match after {`
  `    None => state.policies.values().take(page_size + 1).collect(),`
  `    Some(token) => state.policies.range::<str, _>((Bound::Excluded(token), Bound::Unbounded)).map(|(_, policy)| policy).take(page_size + 1).collect(),`
  `};`
  `let more_remain = window.len() > page_size;`
  `window.truncate(page_size);`
  `let next_token = window.last().filter(|_| more_remain).map(|policy| policy.arn.clone());`

Copy 2 — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_policy_versions.rs:225-239:
  `let mut window: Vec<&crate::state::CatalogPolicyVersion> = match after {`
  `    None => policy.versions.values().take(page_size + 1).collect(),`
  `    Some(version_id) => policy.versions.range((Bound::Excluded(version_id), Bound::Unbounded)).map(|(_, version)| version).take(page_size + 1).collect(),`
  `};`
  `let more_remain = window.len() > page_size;`
  `window.truncate(page_size);`
  `let next_token = window.last().filter(|_| more_remain).map(|version| version.version_id.to_string());`

The same rule, already centralized once in this crate for the record listings — state.rs:386-414:
  `/// This is the ONE place the page of a seq-token listing is cut, so the slice width and`
  `/// the next-token condition cannot drift between the listings`
  `pub fn page_after_seq<'a, T, I>(ordered: I, after: u64, max_results: i32) -> SeqPage<'a, T>`
  `    let more_remain = records.len() > page_size;`
  `    records.truncate(page_size);`
  `    let next_token = records.last().filter(|_| more_remain).map(|record| record.seq().to_string());`
and both handlers' own comments already point at the shared page-width helper: list_policies.rs:175-177 and list_policy_versions.rs:192-194 — "The width one `MaxResults` asks for, from the one place this service decides it ([`crate::state::list_page_size`])" — so the width is shared while the cut around it is not.

Tests that cover both copies and must stay unchanged:
  tests/all/policy_catalog.rs:224 `paging_walks_every_policy_exactly_once`, tests/all/policy_catalog.rs:311 `a_foreign_next_token_is_refused`
  tests/all/policy_versions.rs:482 `paging_reports_every_version_exactly_once_from_state`, tests/all/policy_versions.rs:623 `a_foreign_next_token_is_refused`

## Steps to Reproduce

1. Open services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_policies.rs and read lines 196-210.
2. Open services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_policy_versions.rs and read lines 225-239: the same six-step cut (take page_size+1, more_remain, truncate, token from the last entry iff more remain), differing only in the map and the token rendering.
3. Compare both with crate::state::page_after_seq (state.rs:386-414), whose doc says it is "the ONE place the page of a seq-token listing is cut, so the slice width and the next-token condition cannot drift between the listings" — the two catalog listings sit outside it.
4. Change the more_remain condition or the truncation width in one copy only: cargo test -p service-com-amazonaws-mpa-awsfluffycoreservice still passes for the other listing, showing the copies can drift independently.
