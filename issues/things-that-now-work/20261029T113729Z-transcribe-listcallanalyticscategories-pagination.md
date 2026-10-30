Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3ee3ad8755f7cbca1472e043fe3407ec62e0eff1

# The name page cursor is accepted with a name part no CategoryName can be, although its own doc quotes the @length(1..200) that bounds it

_Filed: 2026-10-29T11:37:29Z_
_Operation: ListCallAnalyticsCategories | Class: pagination_

> _Note: this issue was identified against the code base at commit `3ee3ad8755f7cbca1472e043fe3407ec62e0eff1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListCallAnalyticsCategories` is the one listing of this service paged by NAME. Its cursor is minted by `crate::state::name_page_cursor(scope, &record.name)` — always from a STORED category name, which the model constrains to `@length(min: 1, max: 200)` and `@pattern "^[0-9a-zA-Z._-]+$"` and the boundary validator enforces on `CreateCallAnalyticsCategory`.

The reader, `crate::state::parse_name_page_cursor`, checks the scope prefix and then only that the remainder is NON-EMPTY. So any in-scope token whose name part could not be a category name at all — one carrying characters the pattern forbids (`ListCallAnalyticsCategories|us-east-1|cat!!`), or one longer than the 200 characters the doc itself quotes (`ListCallAnalyticsCategories|us-east-1|` + 500 'z's) — is honoured as a resume point. The handler then skips every category whose name sorts at or below that forged position (list_call_analytics_categories.rs:223-229), so the caller is served HTTP 200 with a page positioned by a token this service never wrote — usually an EMPTY page, which tells a paging client the collection is exhausted — instead of the refusal the codec documents.

This is the gap the sequence-paged sibling in the same module does NOT have: `parse_page_cursor` bounds its decoded position by what the store has actually minted (`(1..=minted_through).contains(sequence)`), precisely so "a cursor THIS listing handed out" is decidable. The name codec applies no corresponding check, though the information to make the same decision is in the same store (the names the Region holds) and the constraint it quotes is in the model.

## Evidence

What the name codec MINTS, and from what — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:3343-3359:
"/// The `NextToken` a listing of records keyed by NAME ALONE hands out to resume\n/// strictly after `name`." ... "pub fn name_page_cursor(scope: CursorScope<'_>, name: &str) -> String {\n    format!(\"{prefix}{name}\", prefix = scope.prefix())\n}"
Its only caller passes a STORED name — src/ops/list_call_analytics_categories.rs:236-238:
"            next_token = page\n                .last()\n                .map(|record| crate::state::name_page_cursor(scope, &record.name));"

What it ACCEPTS, and the constraint its own doc quotes — state.rs:3361-3372:
"/// The record name a `NextToken` resumes after, or `None` for a token the listing\n/// `scope` never handed out — which that listing refuses rather than silently\n/// restarting from the top.\n///\n/// Every name this service keys a record by is non-empty (`CategoryName` is modelled\n/// `@length(min: 1, max: 200)`), so a scope prefix with nothing behind it is a token\n/// this service never wrote.\npub fn parse_name_page_cursor<'t>(token: &'t str, scope: CursorScope<'_>) -> Option<&'t str> {\n    token\n        .strip_prefix(scope.prefix().as_str())\n        .filter(|name| !name.is_empty())\n}"
The doc cites `@length(min: 1, max: 200)`; the filter enforces only the min, and nothing at all of the pattern.

The model constraint the mint side is bounded by — `smithy_query(query="operation", operation="GetCallAnalyticsCategory")` / CategoryProperties:
"CategoryName": {"target": "CategoryName", "length": {"min": 1, "max": 200}, "pattern": "^[0-9a-zA-Z._-]+$"}
so no stored category name can contain '!' or exceed 200 characters, and no cursor this listing minted can carry one.

The strict sibling in the SAME module — state.rs:3337-3340:
"    sequence\n        .parse::<u64>()\n        .ok()\n        .filter(|sequence| (1..=minted_through).contains(sequence))"
with its stated reason at state.rs:3319-3325: "a cursor is only one this listing could have handed out when its sequence has actually been minted for the family that listing reports".

What the forged position does to the page — src/ops/list_call_analytics_categories.rs:223-229:
"        let mut page: Vec<&crate::state::CallAnalyticsCategoryRecord> = store\n            .call_analytics_categories\n            .iter_in(&region)\n            .skip_while(|(name, _)| resume_after.as_deref().is_some_and(|after| *name <= after))"

Test coverage leaves it open — tests/all/call_analytics_category_listing.rs:351-395 (`foreign_tokens`), the table backing `a_next_token_this_service_did_not_hand_out_is_refused` (:399-410): its rows are a bare word, the sequence-cursor format, "this listing's scope with no name behind it", the scope in another Region, "name|", a bare category name, the scope in the wrong case, and "t".repeat(NEXT_TOKEN_CEILING). Every one of them fails the scope prefix or is empty behind it; none carries an IN-SCOPE name that no CategoryName could be, so the suite passes with the hole open and no existing expectation has to change.

## Steps to Reproduce

1. Stand the emulator up with transcribe mounted, as tests/all/call_analytics_category_listing.rs does
2. CreateCallAnalyticsCategory for a few names (e.g. "cat.01" .. "cat.07") with one Rule each
3. POST / with X-Amz-Target: Transcribe.ListCallAnalyticsCategories and body {"NextToken":"ListCallAnalyticsCategories|us-east-1|cat!!"} — accepted: parse_name_page_cursor strips the scope and sees a non-empty remainder, so "cat!!" is used as the resume position even though CategoryName's @pattern ^[0-9a-zA-Z._-]+$ means no stored category is ever named that and name_page_cursor can never have written it
4. Observe HTTP 200 with the categories whose names sort above "cat!!" — a page positioned by a token this service never handed out
5. Repeat with {"NextToken":"ListCallAnalyticsCategories|us-east-1|" + "z".repeat(500)} — also accepted, and because every stored name sorts below it the answer is HTTP 200 with an EMPTY Categories list and no NextToken, which tells a paging client the collection is exhausted
6. Expected per state.rs:3361-3367: 400 BadRequestException "The NextToken in your request is not valid. Use the NextToken returned by your previous ListCallAnalyticsCategories request." — the answer every row of tests/all/call_analytics_category_listing.rs:360-395 already gets
7. Compare the sequence-paged sibling: ListVocabularies with {"NextToken":"ListVocabularies|us-east-1|999999|v1"} IS refused, because parse_page_cursor bounds the decoded position by what the store minted (state.rs:3337-3340)
