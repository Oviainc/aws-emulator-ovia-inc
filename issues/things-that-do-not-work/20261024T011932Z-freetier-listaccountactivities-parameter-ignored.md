Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ListAccountActivities accepts languageCode but never varies the returned titles with it

_Filed: 2026-10-24T01:19:32Z_
_Operation: ListAccountActivities | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAccountActivities$languageCode` is documented as "The language code used to return translated titles." The handler validates the value against the 13-value `LanguageCode` enum and then discards it: the page is built from the single stored English `title` of each catalog row, so `en-US`, `ja-JP`, `fr-FR` and every other modelled code produce byte-identical `activities[].title` values.

Expected: the `title` reported in each `ActivitySummary` reflects the requested language — at minimum the response text must differ for at least one modelled code, as it does on the real service, which returns translated titles.

Actual: the member is accepted (and an out-of-enum value is correctly refused with ValidationException), but it has no observable effect on any response field; the English catalog text is always returned.

Note: the equivalent gap on the sibling reader `GetAccountActivity` is already filed separately (GetAccountActivity/parameter-ignored); this entry covers the list operation, whose stored rows and summary rendering are a different code path.

## Evidence

Model: `ListAccountActivitiesRequest$languageCode` (LanguageCode enum, 13 values) — "The language code used to return translated titles"; `ActivitySummary$title` is @required (smithy_query shape=ActivitySummary). Handler: services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/list_account_activities.rs lines 183-193 validate the code against LANGUAGE_CODE_VALUES and nothing downstream consumes it — the summary built at lines 223-233 clones `activity.title` unconditionally; the stored rows in src/state.rs `seed_activities` (lines 369-449) carry one language only. Tests: tests/all/free_tier.rs `activity_readers_check_the_language_code` asserts only that an out-of-enum code is refused and an in-enum code is accepted on both readers; no test asserts the returned text depends on the code, so the dropped effect is unverified.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. POST X-Amz-Target: AWSFreeTierService.ListAccountActivities with {"maxResults": 10, "languageCode": "en-US"} and record the activities[].title values.
3. POST the same request with {"maxResults": 10, "languageCode": "ja-JP"}.
4. Compare the two responses: the titles are identical, i.e. the requested language had no observable effect.
