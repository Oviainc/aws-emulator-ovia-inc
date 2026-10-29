Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: d626157daccf58583369e714487cbead343a069c

# A ListCompositions nextToken carries only the region, not the filters that select the page, so resuming it under a different filter silently drops compositions

_Filed: 2026-10-29T11:46:22Z_
_Operation: ListCompositions | Class: pagination_

> _Note: this issue was identified against the code base at commit `d626157daccf58583369e714487cbead343a069c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListCompositions` pages over the compositions its two filter members select — `filterByStageArn` and `filterByEncoderConfigurationArn` are applied BEFORE the resume position is compared (`list_compositions.rs:213-229`) — yet the token this listing issues carries only the region as its scope (`state.rs:4711-4719`, `encode_after(COMPOSITION_PAGE_TOKEN_PREFIX, &[region], ...)`).

The crate's own page-token codec states the rule this breaks: the scope a token carries is "the terms of the request that select them", so that a token offered to any other scope "names no position in the records that page holds, whatever store-wide sequence it carries". `filterByStageArn` is exactly such a selecting term, and it is absent from the scope.

Consequence: a caller that pages `filterByStageArn = A` and then offers the resulting `nextToken` with `filterByStageArn = B` (or with no filter at all) is answered HTTP 200 with a SHORT or EMPTY page — every composition of B whose store-wide sequence is below the position A's page ended at is skipped and can never be reached — instead of the `ValidationException` this crate reserves for a token that names no position in the page it is offered to. The crate already carries the correct shape for the listings whose selecting terms are path-like: `ListParticipants` puts `region`, `stageArn` AND `sessionId` in the scope (`list_participants.rs:244-249`).

The same gap exists for `ListIngestConfigurations` and is filed separately.

## Evidence

The rule the crate states for every page token, `src/state.rs:4722-4734`:

    /// The one page-token codec, so the listings cannot encode a position two ways.
    ///
    /// A page position is an ordinal in ONE list of records, and which records a listing
    /// pages over is decided by `scope`: the terms of the request that select them. The
    /// token therefore carries that scope beside the position, and [`decode_after`] reads a
    /// position out of it only for the scope the resuming request itself names — so a token
    /// offered to any other scope names no position in the records that page holds, whatever
    /// store-wide sequence it carries.
    fn encode_after(prefix: &str, scope: &[&str], after_sequence: u64) -> String {

What this listing's token actually scopes, `src/state.rs:4711-4719`:

    pub fn encode_composition_page_token(region: &str, after_sequence: u64) -> String {
        encode_after(COMPOSITION_PAGE_TOKEN_PREFIX, &[region], after_sequence)
    }
    ...
    pub fn decode_composition_page_token(region: &str, token: &str) -> Option<u64> {
        decode_after(COMPOSITION_PAGE_TOKEN_PREFIX, &[region], token)
    }

The filters that DO select which records the page holds, `src/ops/list_compositions.rs:213-229`:

        let matching: Vec<&crate::state::CompositionRecord> = store
            .compositions_in_order(&region)
            .into_iter()
            .filter(|record| {
                input
                    .filter_by_stage_arn
                    .as_deref()
                    .is_none_or(|stage_arn| record.stage_arn == stage_arn)
            })
            .filter(|record| {
                input
                    .filter_by_encoder_configuration_arn
                    .as_deref()
                    .is_none_or(|arn| record.uses_encoder_configuration(arn))
            })
            .filter(|record| resume_after.is_none_or(|after| record.sequence > after))

The handler's own claim about which tokens cannot name a position here, `src/ops/list_compositions.rs:172-175` — it names only the other listings and the other regions, never the other filters:

        // A `nextToken` this listing did not issue names no position, so it is refused
        // rather than read as "start from the beginning" — including a token from this
        // service's other listings, and one this listing issued in ANOTHER region, each
        // of which names a position among records these pages do not hold.

The sibling listing that gets it right, `src/ops/list_participants.rs:241-249`:

        let resume_after = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => Some(
                crate::state::decode_participant_page_token(
                    &region,
                    &input.stage_arn,
                    &input.session_id,
                    token,

Test coverage: `tests/all/page_token_scope.rs:607-624` exercises only the region term for this listing ("The account-wide listings: the region is the whole scope." — `pair("another region", &here.compositions, &there.compositions)`), so no test pins the cross-filter case either way.

## Steps to Reproduce

1. Create one stage A and one stage B, and an encoder configuration, through one region's endpoint.
2. Call StartComposition twice against stage A and twice against stage B (four compositions, A's started first so they take the lower store sequences).
3. Call ListCompositions with filterByStageArn = B and maxResults = 1; keep the returned nextToken (it names the store-wide sequence of B's first composition).
4. Call ListCompositions again with the SAME nextToken but filterByStageArn = A (or with no filter).
5. Observe HTTP 200 with a page that silently omits every composition of A — their sequences are below the position the token names. Expected, per the rule state.rs:4722-4734 states and the refusal list_compositions.rs:172-186 performs for a foreign token: HTTP 400 ValidationException "ListCompositions.nextToken is required to be a pagination token issued by ListCompositions".
