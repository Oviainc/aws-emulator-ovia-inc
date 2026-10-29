Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 8c9af3edcb8acaeda73095133cb6439779d36024

# ListRules still accepts one NextToken it never issued: the minting watermark includes the never-minted all-zeros identifier

_Filed: 2026-10-29T04:44:19Z_
_Operation: ListRules | Class: pagination_

> _Note: this issue was identified against the code base at commit `8c9af3edcb8acaeda73095133cb6439779d36024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRules` refuses a `NextToken` that is not a cursor this endpoint handed out, by asking `ComAmazonawsRbinAmazonrecyclebinState::has_minted_identifier` whether the cursor names a position the store ever minted. That watermark test is INCLUSIVE of the rendering of seed `0` (`identifier <= base62_identifier(self.next_identifier_seed)`), while `mint_identifier` PRE-increments the seed before rendering a candidate — so `base62_identifier(0)` = `"00000000000"` is a position the store can never have handed out, yet it satisfies the watermark for every store state, including a brand-new (seed `0`) one.

Consequence: a client-invented token whose payload is `<this request's region>/00000000000` is accepted as a cursor. `page_from` then walks with `Bound::Excluded("00000000000")`, and because every minted identifier is left-padded base62 and therefore sorts strictly above the all-zeros rendering, the request is answered HTTP 200 with the FIRST page — exactly the "silently restart the walk" outcome the handler's own comment says must not happen, instead of the `ValidationException` / `INVALID_PAGE_TOKEN` the crate already composes for every other token it did not issue.

This is the residual bottom-end off-by-one of the watermark, not the shape-only check that the earlier (fixed) forged-token issue removed: every other never-issued identifier-shaped value is now correctly refused; this single value is not.

## Evidence

The rule the handler states for a token it did not hand out — `src/ops/list_rules.rs:181-185`:
```
        // A `NextToken` this endpoint did not hand out resolves to no cursor at all:
        // only a cursor of THIS Region's walk, naming a position the store really
        // minted, says where to resume. Answering the first page instead would
        // silently restart the walk, and resuming after a position never handed out
        // would answer a page missing every rule that sorts at or before it.
```
and the refusal it is supposed to reach, `src/ops/list_rules.rs:194-197`:
```
                    return Err(ListRulesErrorModeled::ValidationException(
                        crate::state::invalid_page_token(),
                    )
                    .into());
```

The gate it goes through — `src/state.rs:1186-1190`:
```
    pub fn page_cursor(&self, region: &str, token: &str) -> Option<String> {
        let cursor = parse_page_token(token)?;
        (cursor.region == region && self.has_minted_identifier(&cursor.identifier))
            .then_some(cursor.identifier)
    }
```

The watermark's own claim — `src/state.rs:1160-1172`:
```
    /// Whether `identifier` is one this store has already MINTED.
    ///
    /// [`Self::mint_identifier`] renders a seed that only ever rises, and
    /// [`base62_identifier`] renders every seed as the same number of digits in
    /// ascending-byte order, so the renderings this store has handed out are exactly the
    /// identifiers up to and including the rendering of its current seed. An
    /// identifier-shaped value above that watermark names no position this store has
    /// ever reached — nothing could have been minted there — whether or not a rule still
    /// holds it today.
    pub fn has_minted_identifier(&self, identifier: &str) -> bool {
        is_rule_identifier(identifier)
            && identifier <= base62_identifier(self.next_identifier_seed).as_str()
    }
```
and the minting loop that contradicts "the identifiers up to and including the rendering of its current seed" at the BOTTOM end — `src/state.rs:1037-1042`:
```
    pub fn mint_identifier(&mut self) -> String {
        loop {
            self.next_identifier_seed = self.next_identifier_seed.wrapping_add(1);
            let candidate = base62_identifier(self.next_identifier_seed);
```
The seed is incremented BEFORE it is rendered, and the store starts at `pub next_identifier_seed: u64` = 0 (`src/state.rs:1026`, struct `#[derive(Debug, Default)]` at `src/state.rs:1014`), so the rendering of seed 0 (`"00000000000"`) is never a value `mint_identifier` returns — yet it always passes `identifier <= base62_identifier(seed)`.

Where the accepted value is then used — `src/state.rs:1203-1209`:
```
        let lower = match after {
            Some(identifier) => Bound::Excluded(identifier.to_string()),
            None => Bound::Unbounded,
        };
```

Test coverage: `tests/all/list_rules_page_token_authenticity.rs` pins the token-authenticity class, and `tests/all/error_response_bodies.rs` pins the `INVALID_PAGE_TOKEN` body; neither exercises a cursor payload whose identifier is the all-zeros rendering, which is the one value that slips through.

## Steps to Reproduce

1. POST /rules signed for us-east-1 with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} — HTTP 201, one rule now exists.
2. POST /list-rules signed for us-east-1 with {"ResourceType":"EBS_SNAPSHOT"} — HTTP 200, the rule is reported, no NextToken.
3. POST /list-rules signed for us-east-1 with {"ResourceType":"EBS_SNAPSHOT","NextToken":"dXMtZWFzdC0xLzAwMDAwMDAwMDAw"} (base64 of "us-east-1/00000000000" — a cursor naming the rendering of seed 0, which mint_identifier never returns).
4. Expected per src/ops/list_rules.rs:181-185: HTTP 400 ValidationException with Reason=INVALID_PAGE_TOKEN, as for any other cursor the endpoint did not issue. Actual: HTTP 200 and the first page, because has_minted_identifier accepts the all-zeros identifier and Bound::Excluded("00000000000") excludes nothing — the walk is silently restarted.
5. For contrast, replay the same request with a cursor one position higher (base64 of "us-east-1/00000000001") against a store that has minted nothing: it is also accepted, but every identifier-shaped value ABOVE the watermark is correctly refused, showing the defect is the inclusive bottom end of the watermark.
