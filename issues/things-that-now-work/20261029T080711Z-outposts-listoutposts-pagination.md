Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 4c62cef672c36ef491d018c691144f86baa80e9d

# decode_page_token honours boundary spellings encode_page_token never writes (leading zeros), so a token this service did not issue silently truncates a page

_Filed: 2026-10-29T08:07:11Z_
_Operation: ListOutposts | Class: pagination_

> _Note: this issue was identified against the code base at commit `4c62cef672c36ef491d018c691144f86baa80e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged answer in this crate decodes its `NextToken` through the one shared helper `crate::state::decode_page_token`, whose own documentation promises it returns `None` "when it is not a token this service issued for that list — which the caller is told as a `ValidationException`, never as a silently truncated page". Each of the twelve paged handlers repeats that rule in its own comment (`ListOutposts`: "A token this service did not issue for THIS list is a bad request, not an empty page").

The decode does not keep that promise. It validates only the STAMP half and then calls `sequence.parse::<u64>()` on the numeric half, accepting any spelling Rust's integer parser accepts. `encode_page_token` renders the boundary with `format!("{last_sequence}##{kind}")`, which never emits a leading zero, yet `"007##outposts"` — a value the modelled `Token` pattern `^(\d+)##(\S+)$` admits, so it reaches the handler rather than being refused at the boundary — decodes to `Some(7)` and is honoured as this list's resume point. The caller gets HTTP 200 with every Outpost of creation sequence 1..=7 silently omitted, on a token string the emulator could never have minted, while the same route answers 400 `ValidationException` for `"7##sites"`.

The fix belongs in the one shared decode (reject a numeric half that is not the canonical rendering `encode_page_token` writes), not in the twelve handlers. Note that the OTHER tolerant case — a canonical token whose sequence is past the end of the list — is deliberately served as an empty page and is pinned with a stated reason by `tests/all/order_listing.rs:401-413`, so this finding is specifically about a spelling the encoder never produces.

## Evidence

The promise the shared decode makes about itself, and all it actually checks — `services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:6128-6137`:

    /// The page boundary a `kind` token names, or `None` when it is not a token this
    /// service issued for that list — which the caller is told as a
    /// `ValidationException`, never as a silently truncated page.
    pub fn decode_page_token(kind: &str, token: &str) -> Option<u64> {
        let (sequence, stamp) = token.split_once("##")?;
        if stamp != kind {
            return None;
        }
        sequence.parse().ok()
    }

The only rendering the service ever issues — `src/state.rs:6116-6126`:

    /// Encode a page boundary as an opaque `Token`.
    ///
    /// The token carries the creation sequence of the LAST item the page returned, so
    /// the next page is everything created strictly after it ...
    /// The rendering is exactly the modelled `Token` pattern `^(\d+)##(\S+)$`.
    pub fn encode_page_token(kind: &str, last_sequence: u64) -> String {
        format!("{last_sequence}##{kind}")
    }

`{last_sequence}` on a `u64` never renders a leading zero, so `"007##outposts"` is not a token this service issued — yet `"007".parse::<u64>()` is `Ok(7)`.

The handler that states the rule it does not keep — `src/ops/list_outposts.rs:159-172`:

            // The resume point the caller's token names. A token this service did not
            // issue for THIS list is a bad request, not an empty page.
            let after = match input.next_token.as_deref().filter(|token| !token.is_empty()) {
                Some(token) => Some(
                    crate::state::decode_page_token(crate::state::OUTPOST_PAGE, token).ok_or_else(
                        || {
                            ListOutpostsErrorModeled::ValidationException(ValidationException {
                                message: Some(crate::state::page_token_report("ListOutposts")),
                            })
                        },
                    )?,
                ),
                None => None,
            };

and what the decoded boundary then does — `src/state.rs:6242-6244`:

        let mut ordered: Vec<(u64, T)> = records
            .filter(|(sequence, _)| after.is_none_or(|boundary| *sequence > boundary))
            .collect();

The modelled member that lets the value through the boundary validator — smithy_query(query="shape", shape="Token"):

    "Token": {"type": "string", "length": {"min": 1, "max": 2048}, "pattern": "^(\\d+)##(\\S+)$"}

Test coverage today: `tests/all/page_token_refusal_shape.rs:17-22` probes every paged answer only with tokens stamped for the OTHER lists ("The rows span EVERY paged answer this service serves, each probed with a token stamped for each of the OTHER paged answers"), and `tests/all/order_listing.rs:401-413` pins the one tolerant case the crate does justify ("A boundary past the end of the list: nothing left to report, and no token."). No test anywhere sends a non-canonical spelling of a boundary the list DOES hold, so the leading-zero case is untested rather than justified.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. POST /sites with {"Name":"s"}, then POST /outposts eight times against that SiteId, so ListOutposts holds Outposts of creation sequence 1..8.
3. GET /outposts?MaxResults=7 — the response carries NextToken="7##outposts" (the only spelling encode_page_token writes).
4. GET /outposts?NextToken=007%23%23outposts — a value the modelled Token pattern ^(\d+)##(\S+)$ accepts, and a string encode_page_token can never produce.
5. Observe HTTP 200 with only the 8th Outpost reported: the never-issued token was honoured as the boundary 7 and seven Outposts were silently dropped.
6. Contrast with GET /outposts?NextToken=7%23%23sites on the same route, which is refused 400 ValidationException with the page_token_report body — showing the handler does mean to refuse tokens it did not issue.
