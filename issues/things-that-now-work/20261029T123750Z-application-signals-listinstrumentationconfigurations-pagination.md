Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c73cb477601ff7c7855bce46e4f89627c43b834e

# A continuation request that also carries the SyncedAt the previous page handed back drops the cursor and answers Changed:false, so pages 2..N are never served

_Filed: 2026-10-29T12:37:50Z_
_Operation: ListInstrumentationConfigurations | Class: pagination_

> _Note: this issue was identified against the code base at commit `c73cb477601ff7c7855bce46e4f89627c43b834e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListInstrumentationConfigurations` serves two documented mechanisms off the same response — a page cursor (`NextToken`) and an incremental-sync watermark (`SyncedAt`) — and the handler lets the sync gate swallow the cursor.

The handler decodes the request's `NextToken` into `cursor` (src/ops/list_instrumentation_configurations.rs:206-218), then computes `changed` from the request's `SyncedAt` (lines 223-226) and only reaches the pager when `changed` is true (lines 228-234). When `changed` is false the decoded `cursor` is silently discarded and the response carries `LatestConfigurations: null` and `NextToken: null`.

That is exactly what happens to a client that follows the two documented members as documented. Page 1 of a scope holding more configurations than `MaxResults` answers `NextToken` ("Pagination token to continue listing configurations when more results are available") together with `SyncedAt = now` ("The server timestamp to supply on the next sync call"). The client's next call carries that `NextToken` and that `SyncedAt`. Nothing in the scope changed between the two calls, so `instrumentation_changed_since` is false (src/state.rs:4582-4591 compares the scope's `last_changed_at` STRICTLY after `since`, and the last change predates page 1's clock reading), and the handler answers `Changed:false` with no page and no token. The remaining configurations are unreachable: the response hands back a fresh `SyncedAt` too (line 241), so every later sync from that watermark is `Changed:false` as well, and the configurations after page 1 are never reported to that client again until some unrelated create/delete touches the scope.

The handler's own doc states the unchanged-sync rule (lines 165-170) and says nothing about a sync that is also a continuation; `crate::state::page_instrumentation_configurations` is written as a key cursor precisely so paging survives concurrent change (src/state.rs:4593-4599), and that work is bypassed on this path.

## Evidence

MODEL — the two members the client is told to send back, `smithy_query(query="shape", shape="InstrumentationConfigurationsPage")` and `ListInstrumentationConfigurationsRequest`:
- output `NextToken`: "Pagination token to continue listing configurations when more results are available."
- output `SyncedAt`: "The server timestamp to supply on the next sync call."
- input `NextToken`: "Use the token returned by a previous call to retrieve the next page of configurations."
- input `SyncedAt`: "The timestamp from the last successful sync. When provided, the response returns `Changed` as `false` if nothing is new since this time, or returns the latest configurations when changes exist."

HANDLER — the cursor is decoded and then thrown away, services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/list_instrumentation_configurations.rs:206-234:
```
        let cursor = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_cursor(&token_tag, token) {
                Some(cursor) => Some(cursor),
...
        let changed = match input.synced_at.as_ref() {
            None => true,
            Some(since) => store.instrumentation_changed_since(&scope, since),
        };

        let (latest_configurations, next_token) = if changed {
            let (page, next_token) =
                store.page_instrumentation_configurations(&scope, cursor.as_deref(), page_size);
            (Some(page), next_token)
        } else {
            (None, None)
        };
```
and the fresh watermark that makes the loss permanent for that client — same file, line 241: `synced_at: now,`.

WHY `changed` IS FALSE ON THE CONTINUATION — src/state.rs:4577-4591:
```
    /// Whether `scope`'s set has changed since `since`.
    ///
    /// Strictly after: a caller that synced at the instant of the last change already saw
    /// it, which is what makes an unchanged-since poll answer `Changed: false`.
    pub fn instrumentation_changed_since(... ) -> bool {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.last_changed_at)
            .is_some_and(|changed| instant_nanos(&changed) > instant_nanos(since))
    }
```

THE HANDLER'S OWN DOC, which covers only the non-paging case — src/ops/list_instrumentation_configurations.rs:165-170: "`SyncedAt` makes the call an incremental sync: when the scope's set has not changed since the instant the caller supplies, `Changed` is false and the configuration details are omitted ... The response always carries the server instant to supply on the next sync and the interval to wait before it."

TEST COVERAGE — no test sends both members together, so nothing pins this behaviour:
- tests/all/instrumentation_configurations.rs:443-477 (`list_all`) follows `NextToken` to the end and never sets `SyncedAt`.
- tests/all/instrumentation_configurations.rs:718-755 (`synced_at_makes_the_list_an_incremental_sync`) sends `SyncedAt` with NO `NextToken`, over a scope holding one configuration, and asserts `answer["LatestConfigurations"].is_null() && answer["NextToken"].is_null()` — a single-page scope, so the assertion stays true under a fix that honours an in-flight cursor.

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. POST /create-instrumentation-configuration twice in one scope (same InstrumentationType/Service/Environment, same SignalType, two different code locations), so the scope holds 2 configurations.
3. POST /list-instrumentation-configurations with {"InstrumentationType":...,"Service":...,"Environment":...,"MaxResults":1} — 200 with Changed:true, one entry in LatestConfigurations, a NextToken, and a SyncedAt.
4. Do what the two member docs say: POST /list-instrumentation-configurations again with the same scope plus BOTH the NextToken and the SyncedAt from step 3.
5. Observed: 200 with Changed:false, LatestConfigurations absent and NextToken absent — the second configuration is never served, and the response's new SyncedAt makes every later sync answer Changed:false too.
6. Expected: the in-flight cursor is honoured, so the second page is served (the same answer step 4 gives when SyncedAt is omitted).
