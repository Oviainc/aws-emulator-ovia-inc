Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: d626157daccf58583369e714487cbead343a069c

# A ListIngestConfigurations nextToken carries only the region, not the filter that selects the page, so resuming it under a different filter silently drops configurations

_Filed: 2026-10-29T11:46:22Z_
_Operation: ListIngestConfigurations | Class: pagination_

> _Note: this issue was identified against the code base at commit `d626157daccf58583369e714487cbead343a069c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListIngestConfigurations` pages over the configurations its filter members select — `filterByStageArn` and `filterByState` are applied BEFORE the resume position is compared (`list_ingest_configurations.rs:243-258`) — yet the token this listing issues carries only the region as its scope (`state.rs:4693-4705`, `encode_after(INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX, &[region], ...)`).

That contradicts the crate's own page-token codec, whose doc states that the scope a token carries is "the terms of the request that select them" so that a token offered to any other scope "names no position in the records that page holds, whatever store-wide sequence it carries". `filterByStageArn` is exactly such a selecting term and it is not in the scope.

Consequence: a caller that pages `filterByStageArn = A` and then offers the resulting `nextToken` with `filterByStageArn = B` (or with no filter) is answered HTTP 200 with a SHORT or EMPTY page — every ingest configuration of B whose store-wide sequence lies below the position A's page ended at is skipped and unreachable — instead of the `ValidationException` this crate reserves for a token that names no position in the page it is offered to. `ListParticipants` shows the correct shape for the same crate, scoping its token by `region`, `stageArn` and `sessionId` (`list_participants.rs:244-249`).

## Evidence

The rule the crate states for every page token, `src/state.rs:4722-4734`:

    /// A page position is an ordinal in ONE list of records, and which records a listing
    /// pages over is decided by `scope`: the terms of the request that select them. The
    /// token therefore carries that scope beside the position, and [`decode_after`] reads a
    /// position out of it only for the scope the resuming request itself names — so a token
    /// offered to any other scope names no position in the records that page holds, whatever
    /// store-wide sequence it carries.
    fn encode_after(prefix: &str, scope: &[&str], after_sequence: u64) -> String {

What this listing's token actually scopes, `src/state.rs:4693-4705`:

    pub fn encode_ingest_configuration_page_token(region: &str, after_sequence: u64) -> String {
        encode_after(
            INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX,
            &[region],
            after_sequence,
        )
    }
    ...
    pub fn decode_ingest_configuration_page_token(region: &str, token: &str) -> Option<u64> {
        decode_after(INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX, &[region], token)
    }

The filter that DOES select which records the page holds, `src/ops/list_ingest_configurations.rs:240-258`:

        let matching: Vec<&crate::state::IngestConfigurationRecord> = store
            .ingest_configurations_in_order(&region)
            .into_iter()
            .filter(|record| {
                input
                    .filter_by_stage_arn
                    .as_deref()
                    .is_none_or(|stage_arn| record.stage_arn == stage_arn)
            })
            ...
            .filter(|record| resume_after.is_none_or(|after| record.sequence > after))

The handler's own statement of which tokens cannot name a position here — the other listings and the other regions only, never the other filters, `src/ops/list_ingest_configurations.rs:200-203`:

        // A `nextToken` this listing did not issue names no position, so it is refused
        // rather than read as "start from the beginning" — including a token from this
        // service's other listings, and one this listing issued in ANOTHER region, each
        // of which names a position among records these pages do not hold.

The sibling listing that scopes its token by the request's selecting terms, `src/ops/list_participants.rs:241-249`:

                crate::state::decode_participant_page_token(
                    &region,
                    &input.stage_arn,
                    &input.session_id,
                    token,

Test coverage: `tests/all/page_token_scope.rs:607-623` exercises only the region term for this listing ("The account-wide listings: the region is the whole scope." — `pair("another region", &here.ingest_configurations, &there.ingest_configurations)`), so the cross-filter case is pinned by nothing.

## Steps to Reproduce

1. Create stage A and stage B through one region's endpoint.
2. Call CreateIngestConfiguration twice with stageArn = A, then twice with stageArn = B (A's configurations take the lower store sequences).
3. Call ListIngestConfigurations with filterByStageArn = B and maxResults = 1; keep the returned nextToken.
4. Call ListIngestConfigurations again with the SAME nextToken but filterByStageArn = A (or with no filter).
5. Observe HTTP 200 with a page that silently omits every configuration of A, whose sequences lie below the position the token names. Expected, per state.rs:4722-4734 and the refusal list_ingest_configurations.rs:204-218 performs for a foreign token: HTTP 400 ValidationException "ListIngestConfigurations.nextToken is required to be a pagination token issued by ListIngestConfigurations".
