Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 2afcd9ff504122666b9644d9cff6b63f96e63ba3

# ListPolicyVersions judges the resource before the NextToken, the reverse of the order its two sibling resource-scoped listings document as deliberate

_Filed: 2026-10-29T08:30:22Z_
_Operation: ListPolicyVersions | Class: error-response_

> _Note: this issue was identified against the code base at commit `2afcd9ff504122666b9644d9cff6b63f96e63ba3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has three paginated listings that page over ONE named resource's records: `ListSessions` (a team's sessions), `ListResourcePolicies` (a resource's attached policies) and `ListPolicyVersions` (a policy's versions). Two of them read the request's `NextToken` grammar BEFORE they resolve the resource, and both carry a comment saying that order is deliberate — the account boundary, then the request's own shape (filters and token), then whether the resource exists. `ListPolicyVersions` does the opposite: it answers `ResourceNotFoundException` (404) for an unknown `PolicyArn` and only then reads the token.

So one and the same kind of request — a listing of a resource this store does not hold, carrying a `NextToken` that is no token of this service's at all — is answered 400 `ValidationException` (`FOREIGN_NEXT_TOKEN_MESSAGE`) by `ListResourcePolicies` and `ListSessions` and 404 `ResourceNotFoundException` by `ListPolicyVersions`. A client cannot learn from this family which of the two it will get; the three handlers disagree about the precedence of the very same two refusals, both of which are stated once for the whole service in `crate::state` (`foreign_next_token`, `resource_not_found`).

Nothing about the token rule depends on the policy being found: `page_by_key_token` reads the scope out of the token (`policy-versions/<policy arn>:`) before it ever touches the map, so the grammar half of the refusal is decidable at exactly the point the two siblings decide it.

## Evidence

The order the sibling listing states as deliberate — services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/list_sessions.rs:219-239:
```
        // The filters are read before any session is: a request carrying one this
        // operation cannot apply is refused, never answered with a page that silently
        // ignored it. The order of the three refusals is deliberate: the ACCOUNT
        // boundary above answers first, so a caller outside the account learns nothing
        // about this request's shape; then the request's own shape (its filters and its
        // token); then, below, whether the team exists.
        let predicates = compile_filters(input.filters.as_deref().unwrap_or_default())?;
...
        let after = crate::state::token_position(
            crate::state::Listing::Sessions {
                approval_team_arn: &input.approval_team_arn,
            },
            input.next_token.as_deref(),
        )
        .map_err(ListSessionsErrorModeled::ValidationException)?;
```
and the 404 that follows it, list_sessions.rs:244-252:
```
        if state.team(&input.approval_team_arn).is_none() {
            return Err(ListSessionsErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into());
```

The same order, same crate — src/ops/list_resource_policies.rs:200-235:
```
        // The token is this service's own: it names THIS listing and the attachment
        // position the next page starts after. Its grammar is read here, with the
        // request's other shape; that the position was handed out is decided below,
        // against the resource's attachments themselves.
        let after = crate::state::token_position(
            crate::state::Listing::ResourcePolicies {
                resource_arn: &input.resource_arn,
            },
            input.next_token.as_deref(),
        )
        .map_err(ListResourcePoliciesErrorModeled::ValidationException)?;
...
        let Some(attached) = state.resource_policies(&input.resource_arn) else {
            return Err(ListResourcePoliciesErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into());
        };
```

The handler that reverses it — src/ops/list_policy_versions.rs:193-220 (the 404 is decided first; the token is only read inside `page_by_key_token` afterwards):
```
        let Some(policy) = state.policy(&input.policy_arn) else {
            // The one message this service answers every resource lookup that
            // resolves to nothing with (`crate::state::resource_not_found`).
            return Err(ListPolicyVersionsErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into());
        };
...
        let page = crate::state::page_by_key_token(
            listing,
            &policy.versions,
            input.next_token.as_deref(),
            input.max_results,
        )
        .map_err(ListPolicyVersionsErrorModeled::ValidationException)?;
```

That the token's grammar half needs no resource at all — src/state.rs:736-749:
```
pub fn page_by_key_token<'a, K, T>(
    listing: Listing<'_>,
    ordered: &'a BTreeMap<K, T>,
    next_token: Option<&str>,
    max_results: i32,
) -> Result<ListingPage<'a, T>, ValidationException>
...
    let after = match token_key(listing, next_token)? {
```

Test coverage: the provenance suite drives all six listings over tokens that ARE this service's grammar and over foreign ones, always against a resource the store holds, so no existing assertion pins the precedence for an absent resource — tests/all/pagination_token_provenance.rs:243-257 lists the never-a-token rows (`"not-a-token"`, `"1"`, …), and tests/all/policy_versions.rs / tests/all/policy_catalog.rs exercise `ListPolicyVersions` only against policies the catalog holds. The 404 for an unknown `PolicyArn` is pinned on its own (no token) by the fixed issue `mpa-getpolicyversion-error-response`'s sibling assertions, and that row stays unchanged either way.

## Steps to Reproduce

1. Mount the service as tests/all/approval_team_lifecycle.rs does (no state needed: a fresh store seeds only `IdentityCenter/MPAApprovalTeamPolicy`).
2. Call `POST /resource-policies/{ResourceArn}/?List&MaxResults=20&NextToken=not-a-token` with a well-formed ARN in the caller's account that names no stored resource (e.g. `arn:aws:mpa:us-west-2:<account>:approval-team/never-created`). Observe 400 `ValidationException` with `state::FOREIGN_NEXT_TOKEN_MESSAGE` — the token is judged before the resource.
3. Call `POST /policies/{PolicyArn}/?List&MaxResults=20&NextToken=not-a-token` with a `PolicyArn` the catalog does not hold (e.g. `arn:aws:mpa:::aws:policy/IdentityCenter/NeverSeeded`, which satisfies the member's modelled pattern).
4. Observe 404 `ResourceNotFoundException` with `state::RESOURCE_NOT_FOUND_MESSAGE` instead: the same doubly-bad request is classified differently by the third listing of the same family.
5. For the body-bound sibling, repeat step 2's shape on `POST /approval-teams/{ApprovalTeamArn}/sessions/?List` with `{"MaxResults": 20, "NextToken": "not-a-token"}` and a never-created team ARN — 400, as in step 2.
