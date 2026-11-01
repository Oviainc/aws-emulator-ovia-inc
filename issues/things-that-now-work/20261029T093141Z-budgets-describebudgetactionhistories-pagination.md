Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# decode_next_token honours offset spellings encode_next_token never mints, so a token this service never issued is served as a page

_Filed: 2026-10-29T09:31:40Z_
_Operation: DescribeBudgetActionHistories | Class: pagination_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated read of this crate resolves its `NextToken` through the one codec in `crate::state` (`encode_next_token` / `decode_next_token`), and `crate::state::page` answers `None` — which each handler turns into the modelled `InvalidNextTokenException` — for a token that "does not decode".

The codec's own stated rule is that only a token this service MINTED decodes: the marker comment says a token "invented by a caller" must not decode here, and `decode_next_token`'s doc says it decodes "a `NextToken` minted by [`encode_next_token`]" and answers `None` "for anything else". `encode_next_token` mints exactly one spelling per offset: lowercase hex of `budgets:offset:<decimal>` as Rust's `Display` for `usize` writes it (no sign, no leading zeros).

`decode_next_token` is wider than that. It strips the prefix and calls `str::parse::<usize>()`, which accepts an optional leading `+` and any number of leading zeros, and `hex::decode` accepts upper-case hex digits that `hex::encode` never writes. So `hex::encode("budgets:offset:+1")`, `hex::encode("budgets:offset:0001")` and the upper-case hex of a real token all decode to a position the list is then served from, with HTTP 200, even though no call of this service ever handed any of them out. The guard right below it is written on the assumption that a decoded offset came from a mint ("A token is only minted while items remain, so its offset was below the length of the list it was minted for"), so a caller-invented position below the list length slips straight past it: the refusal the handler models for a token it did not issue never happens, and a caller that constructs such a token silently skips the entries before that offset instead of being told the token is invalid.

This affects every paginated read of the service through the one shared helper (`DescribeBudgetActionHistories`, `DescribeBudgetActionsForAccount`, `DescribeBudgetActionsForBudget`, `DescribeBudgetNotificationsForAccount`, `DescribeBudgetPerformanceHistory`, `DescribeBudgets`, `DescribeNotificationsForBudget`, `DescribeSubscribersForNotification`); it is filed once against the first of them, the operation whose handler the checklist reaches first.

## Evidence

The stated rule — services/.../src/state.rs:1558-1560:
    /// Marker the opaque pagination token carries, so a token minted by another
    /// service (or invented by a caller) does not decode here.
    const NEXT_TOKEN_PREFIX: &str = "budgets:offset:";

The one spelling the service mints — src/state.rs:1562-1565:
    /// Encode an offset into the opaque `NextToken` a paginated operation hands back.
    pub fn encode_next_token(offset: usize) -> String {
        hex::encode(format!("{NEXT_TOKEN_PREFIX}{offset}"))
    }

The decoder that is wider than the mint — src/state.rs:1567-1574:
    /// Decode a `NextToken` minted by [`encode_next_token`].
    ///
    /// `None` for anything else — the caller answers that with
    /// `InvalidNextTokenException` ("The pagination token is invalid").
    pub fn decode_next_token(token: &str) -> Option<usize> {
        let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
        decoded.strip_prefix(NEXT_TOKEN_PREFIX)?.parse().ok()
    }
`"+1".parse::<usize>()` and `"0001".parse::<usize>()` both answer `Ok(1)` (unsigned integer parsing accepts a leading `+` and leading zeros), so both decode to offset 1.

The guard that assumes a decoded offset was minted — src/state.rs:1591-1601 (`page`):
        let start = match next_token {
            Some(token) => {
                let offset = decode_next_token(token)?;
                // A token is only minted while items remain, so its offset was below the
                // length of the list it was minted for. An offset at or past the end was
                // never minted for THIS list: it is stale or tampered with, not a last page.
                if offset >= items.len() {
                    return None;
                }
                offset
            }

The handler that turns `None` into the modelled refusal — src/ops/describe_budget_action_histories.rs:251-266:
        let (page, next_token) = crate::state::page(
            &records,
            Some(crate::state::page_size(...)),
            input.next_token.as_deref(),
        )
        .ok_or_else(|| {
            DescribeBudgetActionHistoriesErrorModeled::InvalidNextTokenException(
                InvalidNextTokenException {
                    message: Some("The pagination token is invalid.".to_string()),
                },
            )
        })?;

Existing coverage that stops short of these spellings — tests/all/budget_lifecycle.rs:907-919 enumerates the bad-token forms it asserts on ("empty, not hex, odd-length hex, hex of a payload this service does not mint, a real token with a nibble removed, and a decodable token whose offset is past the end"):
    hex::encode("budgets:offset:not-a-number"),
    hex::encode("budgets:offset:999999"),
and tests/all/budget_action_history.rs:841-855 asserts only `encode_next_token(SEEDED_RECORDS + 10)` and `encode_next_token(SEEDED_RECORDS)` — no row carries a `+N`, a leading-zero or an upper-case-hex spelling, so nothing pins the refusal the codec's own doc promises for them.

## Steps to Reproduce

1. Create a budget and a budget action on it, then call ExecuteBudgetAction / UpdateBudgetAction enough times that the action's history holds at least 2 records (see tests/all/budget_action_history.rs for the seeding shape).
2. Call DescribeBudgetActionHistories with {AccountId, BudgetName, ActionId, MaxResults: 1}: it answers the first record and a NextToken equal to hex("budgets:offset:1").
3. Call it again with NextToken = hex("budgets:offset:+1") (i.e. the hex of the literal string `budgets:offset:+1`). Expected per src/state.rs:1558-1560 and 1567-1570 — a token this service never minted does not decode, so InvalidNextTokenException. Actual: 200 with the page starting at offset 1, because `"+1".parse::<usize>()` answers Ok(1).
4. Repeat with NextToken = hex("budgets:offset:0001") and with the UPPER-CASE hex of the real token: both are also served as pages, although `encode_next_token` writes neither spelling.
5. Repeat against any other paginated read of the service (e.g. DescribeBudgets with several budgets) — they all share `crate::state::page`, so they all honour the same never-minted spellings.
