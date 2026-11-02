Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 307ae70e1176afe8148743d9dbd874f3de53a94d

# The nextToken is a bare list POSITION bound to no billing view, so entries are skipped after a disassociation and a token minted for one view is honoured against another

_Filed: 2026-11-02T05:56:47Z_
_Operation: ListSourceViewsForBillingView | Class: pagination_

> _Note: this issue was identified against the code base at commit `307ae70e1176afe8148743d9dbd874f3de53a94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSourceViewsForBillingView` mints its `nextToken` as the hex of a DECIMAL OFFSET into the stored `source_views` list and resumes by slicing `view.source_views.get(resume_after..)`. Two consequences, both of which the crate's three other paginated reads explicitly state they avoid:

1. **Entries are silently dropped when the list shrinks between pages.** The offset counts entries already delivered, not the entry the page ended on, so a `DisassociateSourceViews` call between two pages shifts every later entry down and the next page starts past them. The handler's own comment admits the list can shrink between pages — it just calls the result "the last page" instead of a lost entry.

2. **The token is scoped to nothing.** No `arn` travels in it, so a token minted while walking view A is accepted as a cut point for view B and silently skips B's first entries. `GetBillingPreferences` — after the fix for its own count-token issue — binds the feature AND the filter scope into every token and refuses a token minted for another walk; this read binds nothing.

The rule the rest of the crate follows is stated three times in `src/`: a page resumes strictly AFTER the entry the previous page ended on, keyed by a VALUE the store is ordered by, precisely so that an insert or a removal between pages cannot make the walk skip or repeat another entry. `source_views` is an ordered `Vec<String>` of ARNs, so the same value-cursor rule is available here.

## Evidence

The offset token and the slice that consumes it — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs:149-151 and :228-244:
```
    hex::encode(returned.to_string().as_bytes())
```
```
        // The page starts where the previous one stopped. An offset past the end
        // of the list (the entries behind it were disassociated since the token
        // was issued) leaves nothing to return, which is the last page.
        let resume_after = match input.next_token.as_deref() {
            Some(token) => decode_page_token(token)?,
            None => 0,
        };
        let remaining = view.source_views.get(resume_after..).unwrap_or_default();
...
            Some(limit) if remaining.len() > limit => (
                &remaining[..limit],
                Some(encode_page_token(resume_after + limit)),
```
Note the comment itself concedes "the entries behind it were disassociated since the token was issued" — i.e. the handler knows the list mutates between pages and still uses a positional cut point.

The rule its three siblings state (same crate):
- services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:335-341: "// The page resumes strictly after the last ARN the previous page\n        // returned, so a view inserted or removed between pages cannot make the\n        // walk skip or repeat another one." then `resume_after` is compared as a VALUE (`view.arn.as_str() <= after`).
- services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs:347-349: "// The page resumes strictly after the entry the previous page ended on,\n        // so an allocation written or removed between pages cannot make the\n        // walk skip or repeat another one."
- services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:402-405: "// The page resumes strictly AFTER the entry the previous page ended on,\n        // so a preference written or removed between two pages cannot make the\n        // walk skip or repeat another one — the rule this service's other\n        // paginated reads follow."

The scoping the same sibling applies and this read omits — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs:231-242:
```
    if stored_feature != feature {
        return Err(invalid_field(
            "nextToken",
            "the pagination token was issued for another feature",
        ));
    }
```
and its doc at :306-315: "A cursor is a position among the entries the request selects, so a token is usable only by a request that selects the same ones: the scope covers every member that narrows the sequence". `ListSourceViewsForBillingView`'s token carries no `arn`, so no such refusal is possible.

The model gives this read the `ValidationException` (400) it would refuse a foreign token with — services/service-com-amazonaws-billing-awsbilling/OPERATIONS.md:143.

Test coverage: tests/all/source_views.rs:284-334 pins only malformed/past-the-end tokens (400 / empty page) and tests/all/source_views_tags_and_policy.rs:516-544 walks every page size over a list that is never mutated mid-walk, so neither the shrink case nor the cross-view token case is covered. The in-crate precedent that the count form is a defect is the already-FIXED issue 20261101T163040Z-billing-getbillingpreferences-pagination.md ("The nextToken is a bare entry COUNT scoped only to the feature, so continuing a walk with different filters (or after an update) silently skips entries"); the open maintainability issue 20261101T113709Z-billing-listsourceviewsforbillingview-maintainability.md is explicitly a byte-for-byte refactor ("leaving every response byte and every existing test unchanged") and does not change this behaviour.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it (with the engine state made writable as tests/all/billing_views.rs:57-63 does, so a CUSTOM view can be created).
2. CreateBillingView(name="V", sourceViews=[s0, s1, s2]) where s0/s1/s2 are three pattern-valid billing-view ARNs, and note the minted ARN V.
3. Call ListSourceViewsForBillingView(arn=V, maxResults=2) — observe [s0, s1] and a nextToken (hex of "2").
4. Call DisassociateSourceViews(arn=V, sourceViews=[s0]) — V now holds [s1, s2].
5. Call ListSourceViewsForBillingView(arn=V, maxResults=2, nextToken=<the token>) — observe an EMPTY page and no nextToken: s2 was never reported by any page of the walk. A value cursor on s1 would have returned [s2].
6. Cross-view reuse: CreateBillingView(name="A", sourceViews=[a0, a1]) and CreateBillingView(name="B", sourceViews=[b0, b1, b2]).
7. Call ListSourceViewsForBillingView(arn=A, maxResults=1) and keep its nextToken.
8. Call ListSourceViewsForBillingView(arn=B, nextToken=<A's token>) — observe 200 with [b1, b2]; b0 is silently skipped and no ValidationException is raised for a token issued for another walk.
