Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 5a6a50abfee118ac44c2855d86c20698e89c1a34

# language query parameter is accepted but never changes the returned check text

_Filed: 2026-10-23T08:56:19Z_
_Operation: ListChecks | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `5a6a50abfee118ac44c2855d86c20698e89c1a34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `language` (RecommendationLanguage: de|en|es|fr|id|it|ja|ko|pt_BR|zh|zh_TW) is documented as "The ISO 639-1 code for the language that you want your checks to appear in" — AWS returns the check `name` and `description` localized into the requested language, so `?language=ja` and `?language=en` yield different text for the same check ARN.

Actual: the handler validates the enum (anything outside it is a 400) and then drops the value: the catalog holds exactly one English copy of each check's `name`/`description`, and `check_summary()` copies it verbatim regardless of `language`. Every language therefore returns byte-identical text, which is observable in the response body.

The same accept-and-drop applies to `language` on GetRecommendation, ListRecommendations and ListRecommendationResources (all four handlers document "`language` is accepted and does not change the text"); a localized copy of the seeded catalog text would close all four.

## Evidence

Smithy model: `ListChecks$language` (@httpQuery(language), enum RecommendationLanguage) doc "The ISO 639-1 code for the language that you want your checks to appear in"; `CheckSummary$name`/`$description` are @required strings. Handler: services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_checks.rs — module doc lines 10-12 state `language` is accepted and does not change the text; `handle()` filters only on pillar/awsService/source and never reads `input.language`. State: src/state.rs `Check{name, description}` holds a single English string per check. Tests: tests/all/recommendation_loop.rs line ~259 asserts only that `?language=ja` does NOT change the result set (it pins the current accept-and-ignore behaviour for filtering, not localized text), so no test asserts localized output.

## Steps to Reproduce

1. Boot the emulator with an account entitled to the Trusted Advisor API.
2. GET /v1/checks?language=en and record the `name`/`description` of a check summary.
3. GET /v1/checks?language=ja and compare: the emulator returns the identical English strings, whereas AWS returns the Japanese check name and description for the same check ARN.
4. Repeat with GET /v1/recommendations?language=ja and GET /v1/recommendations/{arn}?language=ja for the same accept-and-ignore behaviour.
