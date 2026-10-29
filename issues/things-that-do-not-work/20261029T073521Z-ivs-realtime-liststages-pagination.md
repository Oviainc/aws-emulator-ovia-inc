Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# Page tokens are honoured for positions encode_after never wrote ("+N"/leading-zero spellings, and sequences the store never issued)

_Filed: 2026-10-29T07:35:21Z_
_Operation: ListStages | Class: pagination_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every listing of this crate resumes from a position decoded by the single page-token codec `crate::state::decode_after`. That codec checks the base64 framing and the scope marker, then does `.parse().ok()` into `u64` on whatever remains. Rust's `u64::from_str` accepts a leading `+` and leading zeros, and of course any value up to `u64::MAX` — none of which `encode_after` can ever write, since it formats a `u64` the store actually issued (`{after_sequence}`). So a hand-built token whose payload is `<marker>+5`, `<marker>0005`, or `<marker>18446744073709551615` is read as a page position and answered HTTP 200 (a resumed page, or an empty page for a position past the end), instead of the ValidationException the crate's own rule reserves for a token the listing did not issue.

This contradicts the helper's own documented contract and the rule every listing handler states, and it is inconsistent with the sibling case the crate already refuses: a marker followed by NON-numeric text is refused (and pinned by a test), while a marker followed by a numeric spelling the encoder cannot produce is accepted. One decoder, so all ten listings of this service (ListStages, ListCompositions, ListEncoderConfigurations, ListIngestConfigurations, ListStorageConfigurations, ListStageSessions, ListParticipants, ListParticipantEvents, ListParticipantReplicas) behave this way.

## Evidence

Documented contract of the decoder, `src/state.rs:4422-4425`:

    /// The creation sequence a `nextToken` resumes after in `region`'s page, or `None` when
    /// the token was not issued by [`encode_page_token`] for that same scope.
    pub fn decode_page_token(region: &str, token: &str) -> Option<u64> {
        decode_after(PAGE_TOKEN_PREFIX, &[region], token)

What the encoder can write, `src/state.rs:4638-4643`:

    fn encode_after(prefix: &str, scope: &[&str], after_sequence: u64) -> String {
        ::aws_smithy_types::base64::encode(format!(
            "{}{after_sequence}",
            scope_marker(prefix, scope)
        ))

What the decoder accepts, `src/state.rs:4647-4653`:

    fn decode_after(prefix: &str, scope: &[&str], token: &str) -> Option<u64> {
        let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
        decoded
            .strip_prefix(&scope_marker(prefix, scope))?
            .parse()
            .ok()
    }

The rule each handler states for a token it did not issue, `src/ops/list_stages.rs:159-175`:

    // A `nextToken` this service did not issue names no position, so it is
    // refused rather than read as "start from the beginning" ...
            Some(token) => Some(
                crate::state::decode_page_token(&region, token).ok_or_else(|| {
                    crate::state::validation_failed(
                        ListStagesErrorModeled::ValidationException,

The sibling case the crate already refuses, pinned by `tests/all/foreign_page_token_wording.rs:257-260`:

        (
            "base64 of a marker with a non-numeric position",
            "aXZzLXJlYWx0aW1lLXN0b3JhZ2UtY29uZmlndXJhdGlvbi1hZnRlcjpub3Bl".to_string(),
        ),

That suite (lines 285-305) asserts 400 + ValidationException + the one foreign-token sentence for every malformed row, but carries no row for a numeric-but-never-issued position, so nothing pins the accepted spellings.

## Steps to Reproduce

1. Create three stages through one region's endpoint so ListStages has more than one page: `aws ivs-realtime create-stage` three times.
2. Call `ListStages` with `maxResults: 1` and keep the `nextToken` it returns; base64-decode it to see the payload `ivs-realtime-stage-after:9/us-east-1:<sequence>`.
3. Re-encode the same marker with the position rewritten as `+<sequence>` (or `00<sequence>`), and call `ListStages` with that as `nextToken`.
4. Observe HTTP 200 with a resumed page, although `encode_page_token` never issued that token; the crate's rule and `tests/all/foreign_page_token_wording.rs` require 400 ValidationException with "ListStages.nextToken is required to be a pagination token issued by ListStages".
5. Repeat with the marker followed by `18446744073709551615`: HTTP 200 with an empty page instead of the same refusal.
