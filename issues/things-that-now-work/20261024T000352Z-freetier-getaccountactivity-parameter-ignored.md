Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# GetAccountActivity accepts languageCode but never varies title/description with it

_Filed: 2026-10-24T00:03:52Z_
_Operation: GetAccountActivity | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`languageCode` is documented as "The language code used to return translated title and description fields". The handler validates the value against the `LanguageCode` enum and then discards it: the response always reports the single stored English `title` and `description`, so `ja-JP`, `fr-FR` and `en-US` all return byte-identical text.

Expected: the reported `title` and `description` reflect the requested language (at minimum the member must change the response for at least one modelled code, as it does on the real service).

Actual: the member is accepted and has no observable effect on any response field.

## Evidence

Model: `GetAccountActivityRequest$languageCode` (LanguageCode enum, 13 values) — "The language code used to return translated title and description fields"; output `title`/`description` are @required. Handler: src/ops/get_account_activity.rs lines 188-198 validate the code and then nothing downstream consumes it; the stored `Activity` rows in src/state.rs carry one language only (seed_activities, lines 369-449). Tests: tests/all/free_tier.rs `activity_readers_check_the_language_code` asserts only that an out-of-enum code is refused and an in-enum code is accepted — no test asserts the returned text depends on the code, so the dropped effect is unverified.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. Call ListAccountActivities to pick a known activityId.
3. POST GetAccountActivity with {"activityId": "<id>", "languageCode": "en-US"} and record title/description.
4. POST GetAccountActivity with {"activityId": "<id>", "languageCode": "ja-JP"} and compare: the title and description are identical, i.e. the requested language had no effect.
