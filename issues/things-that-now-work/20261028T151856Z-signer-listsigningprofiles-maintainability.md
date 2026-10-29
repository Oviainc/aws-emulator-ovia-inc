Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# The paginated page-size computation is implemented twice (ListSigningProfiles and ListSigningPlatforms)

_Filed: 2026-10-28T15:18:56Z_
_Operation: ListSigningProfiles | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The identical page-size computation — default the absent `maxResults` to the operation's page limit, assert in debug that the value is inside the modelled range, clamp it to `1..=limit`, then convert to `usize` with the same expect message — is written out once in `list_signing_profiles.rs` and once in `list_signing_platforms.rs`, differing only in which limit constant it reads. The two copies can drift (one gaining a correction the other does not), even though both are the same rule about the same modelled `@range(min: 1, max: 25)` member. A single helper taking `(requested: Option<i32>, limit: i32)` in `crate::state` — where both `PROFILE_PAGE_LIMIT` and `PLATFORM_PAGE_LIMIT` already live, each deliberately kept per operation — would leave every response byte and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-signer-wallabyservice/src/ops/list_signing_profiles.rs:201-207:
```
        let requested = input.max_results.unwrap_or(crate::state::PROFILE_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::PROFILE_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::PROFILE_PAGE_LIMIT))
            .expect("a page size clamped into 1..=25 fits in usize");
```
Copy 2 — services/service-com-amazonaws-signer-wallabyservice/src/ops/list_signing_platforms.rs:180-188:
```
        let requested = input
            .max_results
            .unwrap_or(crate::state::PLATFORM_PAGE_LIMIT);
        debug_assert!(
            (1..=crate::state::PLATFORM_PAGE_LIMIT).contains(&requested),
            "maxResults {requested} should have been refused at the boundary"
        );
        let page_size = usize::try_from(requested.clamp(1, crate::state::PLATFORM_PAGE_LIMIT))
            .expect("a page size clamped into 1..=25 fits in usize");
```
The preceding comment block is also duplicated verbatim in both files ("`maxResults` is modelled `@range(min: 1, max: 25)` and the boundary validator refused anything outside it before this handler ran…").

Existing tests that pin the behaviour both copies produce: tests/all/profile_listing_pagination.rs (pages ListSigningProfiles with `?maxResults=…` and `nextToken`, incl. `?maxResults=25`) and tests/all/platform_listing.rs:796-817 ("`maxResults` is held to its modelled range: each published bound is served") plus tests/all/platform_listing.rs:715-760 (paging with `?maxResults=1`/`2`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-signer-wallabyservice/src/ops/list_signing_profiles.rs at lines 195-207.
2. Open services/service-com-amazonaws-signer-wallabyservice/src/ops/list_signing_platforms.rs at lines 174-188.
3. Compare: the comment, the `unwrap_or(<limit>)`, the `debug_assert!` with the same message, the `clamp(1, <limit>)` and the same `expect` string are the same logic twice, parameterised only by the limit constant.
4. Confirm the behaviour is covered by tests/all/profile_listing_pagination.rs and tests/all/platform_listing.rs, so collapsing the two copies into one helper must leave both suites passing unchanged.
