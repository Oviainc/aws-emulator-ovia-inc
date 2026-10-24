Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# CreateQuote never produces QuoteOptions, so a quote carries nothing a caller can order from

_Filed: 2026-10-24T05:23:31Z_
_Operation: CreateQuote | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateQuote` persists and echoes the request (CountryCode, RequestedCapacities, RequestedConstraints, RequestedPaymentOptions, RequestedPaymentTerms, Description) and mints `QuoteId`/status/dates, but hardcodes `quote_options: None` and `ordering_requirements: None`.

Expected (real AWS): the entire point of a quote is that the service answers it with priced, selectable options — `Quote$QuoteOptions`, each `QuoteOption` carrying its own `QuoteOptionIdentifier` (`^oqo-[a-f0-9]{17}$`) along with payment option/term and prices — and with the `OrderingRequirements` the quote must satisfy before an order can be submitted. A client flow is CreateQuote → read `QuoteOptions` → CreateOrder with `QuoteIdentifier` + `QuoteOptionIdentifier`.

Actual: `CreateQuote` (and the subsequent `GetQuote`) return a quote with no `QuoteOptions` at all, so there is no `oqo-…` identifier anywhere in the emulator and the quote→order workflow has nothing to reference; the quote is a pure echo of the request plus a status and an expiry. Even without real price-list data, the emulator should derive one option per requested payment option/term combination (defaulting to the modeled payment options/terms when the request named none) with a minted `QuoteOptionIdentifier`, so the response has the shape a client can act on.

## Evidence

Spec: Smithy model `CreateQuote` output `Quote`, whose members include `QuoteOptions` (list<QuoteOption>, each with `QuoteOptionIdentifier` pattern `^oqo-[a-f0-9]{17}$`) and `OrderingRequirements`; `CreateOrder` input declares `QuoteIdentifier` plus `QuoteOptionIdentifier` (`^oqo-[a-f0-9]{17}$`), which can only ever be obtained from a quote's options. Handler: `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_quote.rs` lines ~252-257 set `quote_options: None, ordering_requirements: None` unconditionally. Test coverage: `tests/all/quote_lifecycle.rs:50-60` lists `QuoteOptions`/`OrderingRequirements` in an `UNREPORTED` constant and asserts they are absent, so no test asserts the AWS behavior.

## Steps to Reproduce

1. Boot the outposts emulator
2. POST /quotes with body {"CountryCode":"US","RequestedCapacities":[...],"RequestedPaymentOptions":["ALL_UPFRONT"],"RequestedPaymentTerms":["THREE_YEARS"]}
3. Inspect the returned Quote: no `QuoteOptions` member is present (and none appears on a later GET /quotes/{QuoteIdentifier})
4. Real AWS returns at least one QuoteOption with a `QuoteOptionIdentifier` (oqo-…) per requested payment option/term, which is the value CreateOrder requires
