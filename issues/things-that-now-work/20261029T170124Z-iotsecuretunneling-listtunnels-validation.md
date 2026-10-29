Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# ListTunnels accepts a nextToken it never minted whenever the suffix happens to parse as hex, against decode_next_token's own documented domain

_Filed: 2026-10-29T17:01:24Z_
_Operation: ListTunnels | Class: validation_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTunnels` states its own rule for a page token the service did not issue: refuse it with a 400 rather than silently serving a page. The rule is implemented by `state::decode_next_token`, whose doc says it answers `None` "when the token was not minted by `encode_next_token`".

The implementation does not compute that. `encode_next_token` can only ever produce `"tnl-"` followed by EXACTLY 16 lowercase hex digits (`{seq:016x}`), but `decode_next_token` strips the prefix and hands the remainder to `u64::from_str_radix(.., 16)`, which accepts any length of digits and either letter case. So token strings this service can never mint — `"tnl-1"`, `"tnl-F"`, `"tnl-2"`, `"tnl-00000000000000000002"` — decode successfully and are treated as a legitimate cursor.

The consequence is the one the handler's comment says must not happen: instead of the 400 `ValidationException` the handler returns for a token it did not issue, a forged or truncated token answers HTTP 200 with a page resumed at an arbitrary position the caller chose (`tunnel.creation_seq < seq`). An error is turned into a success, and a client that garbles or truncates a `nextToken` (or hand-writes one) is silently handed a page that is not the continuation of anything, with no way to tell it apart from a real resumption. All these strings also satisfy the modelled `NextToken` pattern `^[a-zA-Z0-9_=-]{1,4096}$`, so the boundary validator passes them straight into the handler.

This is distinct from the open region-scoping token issue: that one is about a token this service DID mint, in another Region's listing; this one is about a string the service can never mint being accepted as canonical at all, and it is fixed by making the decoder accept only the form the encoder produces.

## Evidence

The only form a token can be minted in — `src/state.rs:212-215` and `src/state.rs:796-800`:

    /// The prefix that marks a `ListTunnels` page token as one this service minted.
    /// The whole token stays inside `NextToken`'s modelled pattern
    /// `^[a-zA-Z0-9_=-]{1,4096}$`.
    const NEXT_TOKEN_PREFIX: &str = "tnl-";

    /// The `nextToken` that resumes a `ListTunnels` page after the tunnel whose
    /// creation sequence is `seq`.
    pub fn encode_next_token(seq: u64) -> String {
        format!("{NEXT_TOKEN_PREFIX}{seq:016x}")
    }

(`{seq:016x}` = exactly 16 lowercase hex digits, zero-padded.)

The decoder's documented domain and the code under it — `src/state.rs:802-807`:

    /// The creation sequence a `nextToken` resumes after, or `None` when the token
    /// was not minted by [`encode_next_token`].
    pub fn decode_next_token(token: &str) -> Option<u64> {
        let digits = token.strip_prefix(NEXT_TOKEN_PREFIX)?;
        u64::from_str_radix(digits, 16).ok()
    }

`u64::from_str_radix` accepts any digit count (1..=16, and longer strings with leading zeros) and both letter cases, so `decode_next_token("tnl-1")`, `decode_next_token("tnl-F")` and `decode_next_token("tnl-00000000000000000002")` all answer `Some(..)` for strings `encode_next_token` cannot produce.

The rule the handler states, and the only branch that enforces it — `src/ops/list_tunnels.rs:141-154`:

    // A page token this service did not mint cannot name a position in the
    // listing. The operation models no error for it, so refuse it as invalid
    // input rather than silently restarting the listing.
    let resume_after = match input.next_token.as_deref() {
        None => None,
        Some(token) => match crate::state::decode_next_token(token) {
            Some(seq) => Some(seq),
            None => {
                return Err(crate::scaffolded::Scaffolded::validation(
                    "nextToken is not a token this service issued",
                ))
            }
        },
    };

and the accepted value is then used as the page cursor — `src/ops/list_tunnels.rs:189-191`:

    // Resume strictly after the position the token names: descending
    // order, so the next page holds the tunnels created before it.
    .filter(|tunnel| resume_after.is_none_or(|seq| tunnel.creation_seq < seq))

The modelled member that lets these strings reach the handler, `smithy_query(query="operation", operation="ListTunnels")`:

    "nextToken": {"target": "NextToken", "httpQuery": "nextToken", "pattern": "^[a-zA-Z0-9_=-]{1,4096}$"}

(`tnl-1`, `tnl-F` and `tnl-00000000000000000002` all match it.)

Test coverage: `tests/all/tunnel_lifecycle.rs:527-544` is the one test of this rule, `list_tunnels_refuses_a_next_token_it_did_not_issue`, and its rows are

    for token in ["not-a-token", "tnl-zzzz", "AAAA"] {

— `"not-a-token"`/`"AAAA"` fail `strip_prefix` and `"tnl-zzzz"` fails the hex parse, so every row exercises the failing path only; no row uses a prefix-correct suffix that parses as hex but is not the 16-digit lowercase form the encoder emits. `tests/all/tunnel_lifecycle.rs:474-524` pages only with tokens the service itself minted. So the accepting path is uncovered.

## Steps to Reproduce

1. Mount the service on an `engine::InMemoryEngine` behind a `MultiServiceRouter` as `tests/all/tunnel_lifecycle.rs` does, signed for one Region.
2. Call `OpenTunnel` three times with `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}`; they take creation sequences 0, 1 and 2.
3. Call `ListTunnels` with `{"maxResults":1}` and observe the minted `nextToken`: it is `tnl-` followed by 16 lowercase hex digits (e.g. `tnl-0000000000000002`), the only form `state::encode_next_token` can produce (`src/state.rs:798-800`).
4. Now call `ListTunnels` with `{"nextToken":"tnl-2"}` — a string the service can never have minted. Expected per the handler's own rule (`src/ops/list_tunnels.rs:141-143`) and per `decode_next_token`'s documented domain (`src/state.rs:802-803`): HTTP 400 `ValidationException` with "nextToken is not a token this service issued". Actual: HTTP 200 with a page of the tunnels whose `creation_seq < 2`.
5. Repeat with `{"nextToken":"tnl-F"}` (uppercase hex, which `encode_next_token` never emits) and `{"nextToken":"tnl-00000000000000000002"}` (20 digits): both answer 200 with a resumed page instead of the refusal, so a client that truncates, re-cases or hand-writes a cursor silently gets a page that continues nothing.
6. Contrast with `{"nextToken":"tnl-zzzz"}` (the row `tests/all/tunnel_lifecycle.rs:532` covers), which is refused with 400 — the refusal depends on whether the forged suffix happens to be hex, not on whether the service minted the token.
