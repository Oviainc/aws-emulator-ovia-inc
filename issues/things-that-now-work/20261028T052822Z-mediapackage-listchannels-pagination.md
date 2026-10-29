Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# A nextToken this service never issued is accepted as a page cursor, against the rule decode_page_token's own doc states

_Filed: 2026-10-28T05:28:22Z_
_Operation: ListChannels | Class: pagination_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three paged listings (ListChannels, ListOriginEndpoints, ListHarvestJobs) resolve `nextToken` through `crate::state::decode_page_token`, whose documentation states that it answers `None` for a token this service did not issue so the caller is told about it rather than silently served a page. The implementation checks only two things: that the text is even-length hexadecimal and that the decoded bytes are valid UTF-8. Any such string is therefore accepted as a resume key, including one no call of this service ever minted.

The decoded value is then used as the `>=` lower bound of the page (`crate::state::paginate`), so a forged-but-hex token silently SKIPS every record whose id sorts below it and the listing answers 200 with a short page the caller cannot tell from a complete one. A client that received such a token from another source (or built one itself) is given an arbitrary slice of the Region's channels instead of the refusal the handler comment promises.

The crate already has the refusal (`crate::state::invalid_page_token`, "is not a pagination token this service issued"), and the existing test only exercises tokens that are not hex at all, so the gap is in which tokens reach that refusal, not in whether one exists.

## Evidence

The rule the crate states, `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:2785-2792`:

    /// Decode a paging token back to the key its page starts at.
    ///
    /// `None` for a token this service did not issue, which the caller is told about rather
    /// than silently served page one for.
    pub fn decode_page_token(token: &str) -> Option<String> {
        let bytes = hex::decode(token).ok()?;
        String::from_utf8(bytes).ok()
    }

Nothing there ties the decoded value to a key this service minted, so `decode_page_token("6161")` answers `Some("aa")` — a token no call issued.

The handler that relies on that promise, `src/ops/list_channels.rs:157-165`:

    // An opaque token this service did not issue is refused rather than silently
    // served as page one, which would make a paging client loop over page one.
    let start = match input.next_token.as_deref() {
        None => None,
        Some(token) => Some(
            crate::state::decode_page_token(token)
                .ok_or_else(crate::state::invalid_page_token)?,
        ),
    };

and the start is applied as a bare lower bound, `src/state.rs:2820-2823`:

    let mut remaining = keys
        .into_iter()
        .filter(|key| start.is_none_or(|from| *key >= from));

The same five lines appear in `src/ops/list_origin_endpoints.rs:164-170` and `src/ops/list_harvest_jobs.rs:184-190`.

Test coverage: `tests/all/channel_lifecycle.rs:1161-1184`, `a_pagination_token_this_service_did_not_issue_is_refused`, drives only `["not-a-token", "abc", "zz", "%%%"]` — none of them an even-length hex string — so a well-formed forged token is untested.

## Steps to Reproduce

1. CreateChannel with Id="alpha", then CreateChannel with Id="zulu" in the same Region.
2. GET /channels?nextToken=7a (hex of "z") — a token no ListChannels call ever returned.
3. Observe HTTP 200 with only the "zulu" channel and no indication that "alpha" was skipped, instead of the 422 UnprocessableEntityException / "nextToken is not a pagination token this service issued" the handler comment and decode_page_token's doc state.
4. Repeat against GET /origin_endpoints?nextToken=7a and GET /harvest_jobs?nextToken=7a for the same result.
