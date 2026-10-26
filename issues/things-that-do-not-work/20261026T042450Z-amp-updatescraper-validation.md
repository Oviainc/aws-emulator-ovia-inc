Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# UpdateScraper stores an ampConfiguration destination naming a workspace this account does not hold, a reference CreateScraper refuses

_Filed: 2026-10-26T04:24:50Z_
_Operation: UpdateScraper | Class: validation_

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateScraper` resolves the `ampConfiguration.workspaceArn` of the destination and refuses, with the modelled `ResourceNotFoundException`, an ARN in the emulator's own account that names no stored workspace. `UpdateScraper` — which carries the same optional `destination` member and models the same `ResourceNotFoundException` — assigns it to the stored scraper with no resolution at all, so a scraper can be pointed at a workspace that does not exist and `DescribeScraper` then reports that dangling destination. The helper the create path uses (`destination_workspace_arn` + `crate::state::names_this_account` + `store.taggable_of_arn`) already exists in this crate; the update path simply skips it.

## Evidence

src/ops/create_scraper.rs:245-272 applies the check:
```
        // A scraper's destination is a REFERENCE to another resource, and
        // `ResourceNotFoundException` is modelled here for exactly that: "The request
        // references a resources that doesn't exist." ...
        if let Some(workspace_arn) = destination_workspace_arn(&input.destination) {
            if crate::state::names_this_account(workspace_arn)
                && !matches!(
                    store.taggable_of_arn(workspace_arn),
                    Some(crate::state::TaggedResource::Workspace(_))
                )
            {
                return Err(CreateScraperErrorModeled::ResourceNotFoundException(
```
src/ops/update_scraper.rs:244-246 stores it unchecked:
```
            if let Some(destination) = input.destination {
                scraper.destination = destination;
            }
```
UpdateScraper declares the error it would need — OPERATIONS.md (UpdateScraper): "**Errors:** ... `ResourceNotFoundException (HTTP 404)` ...", documented "The request references a resources that doesn't exist." (src/ops/update_scraper.rs:93-94).

Test coverage note: tests/all/scraper_update.rs:194-230 ("every updatable member at once") currently updates the destination to `arn:aws:aps:us-west-2:{emulator_account_id}:workspace/ws-99999999-...`, a workspace the store does not hold, and asserts the value is stored — that row's ARN would have to name a seeded workspace once the check is added; its stated reason (line 25, "every updatable member") is about per-member replacement, not about destination resolution.

## Steps to Reproduce

1. Create a scraper whose destination is an existing workspace and let it settle to ACTIVE
2. PUT /scrapers/{scraperId} with {"destination": {"ampConfiguration": {"workspaceArn": "arn:aws:aps:us-west-2:<emulator account>:workspace/ws-99999999-9999-9999-9999-999999999999"}}}
3. Observe 202 instead of the 404 ResourceNotFoundException the same ARN earns from POST /scrapers
4. GET /scrapers/{scraperId} — the scraper now reports a destination workspace that does not exist
