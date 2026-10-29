Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: f0727557aa604deafa81f0cee75b9e46b63ce964

# A NextToken whose position is spelled non-canonically (leading zeros, or a leading '+') is accepted and answered with a page, although no listing ever hands one out

_Filed: 2026-10-29T05:36:05Z_
_Operation: ListIdentitySources | Class: pagination_

> _Note: this issue was identified against the code base at commit `f0727557aa604deafa81f0cee75b9e46b63ce964`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this service reads its `NextToken` through the one token reader in `crate::state`, and the module states the provenance rule that reader exists to enforce: a token this listing cannot read, a token another listing issued, and a token naming a record this listing does not hold are all REFUSED with `foreign_next_token()` (400 `ValidationException`, `FOREIGN_NEXT_TOKEN_MESSAGE`) rather than answered with a page.

The key half of that rule is decided by `str::parse`, which accepts spellings this service never mints. `position_token`/`listing_token` render a position as `u64::to_string()` (and a policy version id as `i32::to_string()`), i.e. canonical decimal with no sign and no padding — so `identity-sources:01`, `identity-sources:+1`, `approval-teams:+2`, `sessions/<team arn>:01`, `resource-policies/<arn>:+1` and `policy-versions/<policy arn>:+1` are values no page of any listing has ever handed out. `token_position` nevertheless parses `"01"` and `"+1"` as the position `1` (Rust's integer `FromStr` accepts an optional `+` and any number of leading zeros), and `page_after_position` then finds that a record with seq `1` IS held, so `handed_out` is true and the listing answers 200 with the page after it. `page_by_key_token` has the same hole for `ListPolicyVersions`, whose key is an `i32` version id.

So a client that forged or hand-edited a cursor is served a page from a token the service never issued — exactly the outcome the module's provenance rule and the `pagination_token_provenance` suite exist to prevent, and the same class as the already-fixed "a NextToken this service never issued is accepted when it happens to be numeric". The crate's other reader of a decimal id in caller-supplied text already closes this spelling hole deliberately: `parse_policy_version_arn` refuses a signed spelling byte by byte before parsing, with a comment saying why.

All six listings share the reader; I file it on `ListIdentitySources` because `(ListApprovalTeams, pagination)` is already taken by the earlier, differently-caused issue in the same reader.

## Evidence

The rule, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:159-164:
```
//! A token is ONE listing's, so it carries the listing that issued it — the operation,
//! and the resource it pages over where it pages over one ([`Listing`],
//! [`listing_token`]) — and is only resumable there: a token this listing cannot read, a
//! token another listing issued, and a token naming a record this listing does not hold
//! are all refused ([`token_key`], [`page_after_position`], [`page_after_key`]) rather
//! than answered with a page.
```

What this service actually MINTS, state.rs:528-530 and state.rs:561-563:
```
pub fn listing_token(listing: Listing<'_>, key: &str) -> String {
    format!("{}{TOKEN_SEPARATOR}{key}", listing.scope())
}
```
```
pub fn position_token(listing: Listing<'_>, position: u64) -> String {
    listing_token(listing, &position.to_string())
}
```

What it ACCEPTS, state.rs:567-575 — the only check on the key is that `parse` succeeds, and `u64::from_str` accepts `"+1"` and `"01"`:
```
pub fn token_position(
    listing: Listing<'_>,
    next_token: Option<&str>,
) -> Result<Option<u64>, ValidationException> {
    match token_key(listing, next_token)? {
        None => Ok(None),
        Some(key) => key.parse::<u64>().map(Some).map_err(|_| foreign_next_token()),
    }
}
```
and the same for a catalog key, state.rs:745-748 (`K = i32` for `ListPolicyVersions`):
```
    let after = match token_key(listing, next_token)? {
        None => None,
        Some(key) => Some(key.parse::<K>().map_err(|_| foreign_next_token())?),
    };
```

Why a parsed-but-never-minted position is then SERVED instead of refused, state.rs:626-636 — holding the record is the whole test:
```
    let mut handed_out = after.is_none();
    let mut records: Vec<(u64, &'a T)> = Vec::new();
    for (position, record) in ordered {
        match after {
            Some(after) if position <= after => handed_out |= position == after,
            _ => records.push((position, record)),
        }
    }
    if !handed_out {
        return Err(foreign_next_token());
    }
```

The call site this is filed on, src/ops/list_identity_sources.rs:185-191:
```
        let page = crate::state::page_by_seq_token(
            crate::state::Listing::IdentitySources,
            state.identity_sources_in_creation_order(),
            input.next_token.as_deref(),
            input.max_results,
        )
        .map_err(ListIdentitySourcesErrorModeled::ValidationException)?;
```

The sibling reader in this same crate that DOES close the spelling hole, state.rs:2338-2342:
```
        // `[\d]+` in the pattern, and `PolicyVersionId` is `range(min: 1)`, so a signed
        // or out-of-range value names no version.
        if !version.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
```
and a test row pins that refusal for the signed spelling: tests/all/resource_policies.rs:730 — `"arn:aws:mpa:::aws:policy/IdentityCenter/P/+1".to_string(),`.

Test coverage: the provenance suite drives every listing but only over canonical renderings and bare values, so this spelling is untested. tests/all/pagination_token_provenance.rs:194-236 builds its "in grammar but never handed out" rows through `state::position_token(listing, 0)` / `state::position_token(listing, 999_999)` / `state::listing_token(listing, "0")` (canonical, and positions no record holds), and tests/all/pagination_token_provenance.rs:243-257 lists the never-a-token rows as bare, unscoped values:
```
        ("text that is no token at all", "not-a-token".to_string()),
        ("a bare position", "1".to_string()),
        ("a bare position of nothing", "0".to_string()),
        ("a bare position past every record", "999999".to_string()),
        ("a bare negative position", "-1".to_string()),
        ("a bare position with trailing text", "1x".to_string()),
        ("a bare fractional position", "1.5".to_string()),
```
No row sends a scoped token whose key is a non-canonical spelling of a position the listing HOLDS, so refusing it leaves every existing assertion unchanged.

## Steps to Reproduce

1. Record the caller as the organization's management account so the gated create is served (as tests/all/approval_team_lifecycle.rs:194 does), then `POST /identity-sources` twice so the store holds identity sources at creation-order positions 1 and 2.
2. `POST /identity-sources/?List&MaxResults=1` and observe `NextToken: "identity-sources:1"` — the only spelling this listing ever mints for position 1.
3. `POST /identity-sources/?List&MaxResults=1&NextToken=identity-sources:01` (or `...&NextToken=identity-sources:%2B1`).
4. Observe HTTP 200 with the second source — a page resumed from a token no listing handed out; expected 400 `ValidationException` with `state::FOREIGN_NEXT_TOKEN_MESSAGE`, as for every other never-issued token.
5. Repeat the same spelling trick on the other listings that share the reader: `POST /approval-teams/?List&NextToken=approval-teams:01`, `POST /resource-policies/{ResourceArn}/?List&NextToken=resource-policies/{ResourceArn}:01`, `ListSessions` with `{"NextToken": "sessions/{ApprovalTeamArn}:01"}`, and `POST /policies/{PolicyArn}/?List&NextToken=policy-versions/{PolicyArn}:+1` — each answers a page instead of the refusal.
