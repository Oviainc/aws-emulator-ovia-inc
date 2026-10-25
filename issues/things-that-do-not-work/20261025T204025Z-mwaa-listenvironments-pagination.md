Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0

# Scope the ListEnvironments page cursor to the Region that issued it

_Filed: 2026-10-25T20:40:25Z_
_Operation: ListEnvironments | Class: pagination_

> _Note: this issue was identified against the code base at commit `5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEnvironments` is Region-scoped: the handler lists only `names_in_region(&region)` for the Region the request's SigV4 credential scope names. Its pagination cursor, however, encodes nothing but the environment NAME the page ended on, and `decode_cursor` accepts any well-formed cursor regardless of which Region's listing minted it.

So a `NextToken` minted by a listing signed for Region A is accepted by a listing signed for Region B, where it is interpreted as a position in B's own sorted name list: every environment of Region B whose name sorts at or before the cursor name is silently dropped from the answer, and the walk reports no error. The listing claims to be complete (it eventually answers with no `NextToken`), so the caller cannot tell the page set was truncated.

This contradicts the crate's own scoping rule everywhere else — even the ARN-addressed tagging lookups, whose identifier is globally unique, are refused when they come from another Region — and it contradicts the cursor's own documented guarantee that resuming "never repeats or skips an environment". The cursor only has to carry (and be checked against) the Region it was issued for, the same way `environment_by_arn` filters on `record.region`.

## Evidence

The listing is per-Region — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/list_environments.rs:140-142: "An environment lives in the Region it was created in, and this endpoint serves one Region: the listing names that Region's environments and no others." / `let region = crate::state::request_region(ctx);`, and list_environments.rs:165-166: `let names = store.names_in_region(&region); let page = crate::state::page_of(names, after.as_deref(), input.max_results);`.

The cursor carries only the name — src/state.rs:1317 `const CURSOR_PREFIX: &str = "mwaa-after-environment:";` and src/state.rs:1329-1331 `pub fn encode_cursor(name: &str) -> String { ::hex::encode(format!("{CURSOR_PREFIX}{name}")) }`; src/state.rs:1334-1348 `decode_cursor` accepts any token carrying that prefix and returns the bare name, with no Region to check against the request. list_environments.rs:146 feeds it straight into `page_of`.

Its own doc claims what the missing scoping breaks — src/state.rs:1322-1324: "The cursor names a POSITION in the name ordering rather than an offset, so resuming never repeats or skips an environment when one is created or deleted between pages."

The sibling lookup in the same module DOES reject a cross-Region identifier — src/state.rs:1044-1047: "The lookup is scoped to the Region the request was signed for, like every name-addressed one: an ARN carries its own Region, so an ARN minted in another Region resolves to nothing here rather than reaching across Regions.", implemented at src/state.rs:1053-1057 with `.filter(|record| record.region == region)`.

Test coverage: tests/all/environment_lifecycle.rs:1091-1139 pages the listing exhaustively but only in the single Region its harness signs for; tests/all/environment_region_scoping.rs:291-306 (`a_listing_names_only_the_environments_of_the_region_it_is_signed_for`) lists four populated Regions but never passes a `NextToken`, so no test exercises (or pins) a cursor crossing Regions.

## Steps to Reproduce

1. Boot the emulator.
2. Create two environments signed for us-east-1: `PUT /environments/Zeta` and `PUT /environments/Zulu` (with the required members).
3. Create one environment signed for eu-west-1: `PUT /environments/Alpha`.
4. Send `GET /environments?MaxResults=1` signed for us-east-1 -> `Environments: ["Zeta"]` plus a `NextToken` (the hex of `mwaa-after-environment:Zeta`).
5. Replay that same token against the other Region: `GET /environments?NextToken=<token>` signed for eu-west-1. Expected: the token is not one this Region's listing issued, so it is refused (or resumes eu-west-1's own ordering). Observed: 200 with `Environments: []` and no `NextToken` — 'Alpha' sorts before 'Zeta' and is silently skipped, so the eu-west-1 walk reports an empty, apparently complete listing for a Region that holds one environment.
