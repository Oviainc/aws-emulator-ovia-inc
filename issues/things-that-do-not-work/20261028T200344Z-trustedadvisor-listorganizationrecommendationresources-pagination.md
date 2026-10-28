Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 8135e4312fd85a54e50f448892daaf011c09f669

# Pagination scope is built by unescaped concatenation, so an unconstrained regionCode can forge another filter set's cursor scope

_Filed: 2026-10-28T20:03:44Z_
_Operation: ListOrganizationRecommendationResources | Class: pagination_

> _Note: this issue was identified against the code base at commit `8135e4312fd85a54e50f448892daaf011c09f669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every list of this service builds its cursor scope with `crate::state::list_scope`, which concatenates `<separator><name>=<value>` for each filter that is present. The value is inserted VERBATIM — no escaping, and no rejection of a value containing the separator characters the function itself uses. `ListOrganizationRecommendationResources` passes four filters in the fixed order `status`, `exclusionStatus`, `regionCode`, `affectedAccountId`; of these, `regionCode` is modelled as a plain unconstrained `String` httpQuery (no @length, no @pattern), so a caller may put the scope separator `\u{1e}` and the text `affectedAccountId=<id>` inside it.

The result is that two DIFFERENT filter sets produce the IDENTICAL scope string:

* A: `regionCode="us-east-1"`, `affectedAccountId="111122223333"`
* B: `regionCode="us-east-1\u{1e}affectedAccountId=111122223333"`, `affectedAccountId` absent

A `nextToken` this service issued for read A is therefore ACCEPTED when replayed on read B, whose matching set is empty (no stored resource's `regionCode` equals the injected string) — so the service answers 200 with an empty page where its own stated rule is to answer the modelled `ValidationException` ("Such a token is refused as malformed instead"). The same forging is reachable on `ListRecommendationsForResource`, whose scope embeds the request's `awsResourceArn`, whose modelled pattern ends in `([:/].+)*$` and so admits the separator characters as well.

The defect is the ambiguity of the scope encoding: the function that exists precisely to make a cursor unreplayable across filter sets produces a non-injective encoding of the filter set.

## Evidence

The rule the crate states for exactly this — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:937-946:
```
/// The cursor scope for one read of a filtered list: the list's name (and, for a
/// sub-resource list, the parent it was taken under) plus every filter in force,
/// in the order the handler lists them.
///
/// The filters belong in the scope because they decide the SEQUENCE the cursor
/// points into: resuming a token issued under one filter set against another
/// would skip to a key that sequence orders differently, so a caller following
/// tokens would not see every entry exactly once. Such a token is refused as
/// malformed instead.
```
and the construction that breaks it, src/state.rs:947-958 (the value is concatenated raw, with the separator the scope itself delimits on):
```
pub fn list_scope(list: &str, filters: &[(&str, Option<&str>)]) -> String {
    let mut scope = list.to_string();
    for (name, value) in filters {
        if let Some(value) = value {
            scope.push(SCOPE_SEPARATOR);
            scope.push_str(name);
            scope.push('=');
            scope.push_str(value);
        }
    }
    scope
}
```
(`const SCOPE_SEPARATOR: char = '\u{1e}';` — src/state.rs:935, whose comment claims "no id, ARN or scope this service builds can contain it", which is true only of what the service builds, not of the request value it splices in.)

The handler that makes it reachable, with the free-form filter placed before a constrained one — src/ops/list_organization_recommendation_resources.rs:296-313:
```
        // The cursor's scope names this organization recommendation AND this
        // read's filters, so a token issued for one organization
        // recommendation's resources (or under another filter set) cannot
        // resume another sequence.
        let page = crate::state::paginate(
            &crate::state::list_scope(
                &format!("organizationRecommendationResources/{identifier}"),
                &[
                    ("status", input.status.as_deref()),
                    ("exclusionStatus", input.exclusion_status.as_deref()),
                    ("regionCode", input.region_code.as_deref()),
                    ("affectedAccountId", input.affected_account_id.as_deref()),
                ],
            ),
```

The model confirms `regionCode` carries no constraint while `affectedAccountId` does — smithy_query(query="operation", operation="ListOrganizationRecommendationResources"):
```
"regionCode": {"target": "String", "httpQuery": "regionCode"},
"affectedAccountId": {"target": "AccountId", "httpQuery": "affectedAccountId", "length": {"min": 12, "max": 12}, "pattern": "^\\d+$"}
```

The acceptance path that then cannot tell the two reads apart — src/state.rs:973-981:
```
pub fn decode_token(scope: &str, token: &str) -> Result<String, MalformedToken> {
...
    if token_scope != scope {
        return Err(MalformedToken);
    }
```

Test coverage: tests/all/organization_recommendation_resources.rs is the suite that pins this list's filters and cursor; no test there passes a `regionCode` containing a separator, so nothing pins the ambiguous encoding, and making the encoding injective (escaping the value, or refusing a filter value that contains a separator) changes no existing response.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate admits calls (src/state.rs:182).
2. GET /v1/organization-recommendations/<org-rec-arn>/resources?regionCode=us-east-1&affectedAccountId=<emulator account>&maxResults=1 and copy the `nextToken` from the response.
3. Replay that token on a DIFFERENT filter set whose scope collides: GET /v1/organization-recommendations/<org-rec-arn>/resources?regionCode=us-east-1%1EaffectedAccountId%3D<emulator account>&nextToken=<token> (the `%1E` is the SCOPE_SEPARATOR, and `affectedAccountId` is now absent from the request).
4. Observe HTTP 200 with an empty `organizationRecommendationResourceSummaries`, instead of the modelled ValidationException ("nextToken is not a pagination token this service issued for this list") that src/state.rs:944-945 says such a token must be refused with.
5. Repeat on ListRecommendationsForResource by putting `%1Estatus%3Dwarning` at the end of the `awsResourceArn` path label (its modelled pattern ends `([:/].+)*$`), replaying a token issued for the same ARN with `status=warning`.
