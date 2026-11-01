Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6eca256d767263dcce8cda9bce6eb5315c5aad3d

# A NextToken carries only the listing kind, not the scope it was issued for, so a token from another OperationId silently skips stored results

_Filed: 2026-10-29T11:27:39Z_
_Operation: ListSubCheckResults | Class: pagination_

> _Note: this issue was identified against the code base at commit `6eca256d767263dcce8cda9bce6eb5315c5aad3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged listing of this crate shares one token codec (`crate::state::encode_page_token` / `decode_page_token`). The token is `hex(<listing prefix> + <item key>)`, so the ONLY thing it is bound to is the listing KIND (`Listing::SubCheckResults`, `Listing::Operations`, …) — never the scope the listing was taken over (the `OperationId`, `SubCheckResultId` or `ApplicationId` the request named).

For the listings that are scoped to one parent record this means a token issued by one scope is accepted by another, where its key means nothing, and `page` then resolves it by `skip_while(key < from)`: every item of the requested scope that sorts below the foreign key is silently dropped from the page, with no error and (when the foreign key sorts above everything the scope holds) no `NextToken` either, so the walk terminates having reported a strict subset of what is stored.

`ListSubCheckResults` is the clearest case — its keys are `SubCheckResultId` UUIDs, so a token issued for run A applied to run B drops an arbitrary prefix of run B's sub-check results. The same codec gives the same defect to `ListSubCheckRuleResults` (a `RuleResultId` from another sub-check), `ListOperationEvents` (a zero-padded ordinal from a longer operation, which can empty the page entirely), `ListOperations` / `ListConfigurationCheckOperations` (an `OperationId` key from another application) and `ListComponents` / `ListDatabases` (an `<ApplicationId>/…` key from another application).

That is exactly the failure the codec's own doc says the token marking exists to prevent, and it contradicts the pager's own guarantee that a caller following tokens to exhaustion sees every item. It also contradicts the handler's own stance one screen earlier, where an `OperationId` that resolves to nothing is REFUSED rather than answered with an empty list "which would report an operation that happened and passed no milestone".

## Evidence

The rule the crate states for its tokens — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:4532-4534:
```
/// A token is opaque to a client, but it still names a position in ONE listing: a token
/// handed out by one must not resume another, whose keys mean something else. So the
/// listing is part of what the token marks ([`Listing::token_prefix`]).
```
What the token actually carries — state.rs:4629-4631 and state.rs:4634-4639:
```
fn encode_page_token(key: &str, listing: Listing) -> String {
    ::hex::encode(format!("{}{key}", listing.token_prefix()))
}
...
fn decode_page_token(token: &str, listing: Listing) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(listing.token_prefix())
        .map(str::to_string)
}
```
(only the listing prefix is checked; no `OperationId`/`ApplicationId` is in the token)

How a decoded key is applied — state.rs:4602-4605, under a doc (state.rs:4580-4583) promising "a caller that follows tokens to exhaustion sees every item exactly once":
```
let mut rest = all
    .into_iter()
    .skip_while(|(key, _)| start.as_deref().is_some_and(|from| key.as_str() < from));
let items: Vec<T> = rest.by_ref().take(limit).map(|(_, item)| item).collect();
```

The scoped listing that hands such a key out — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_sub_check_results.rs:183-194:
```
let all: Vec<(String, SubCheckResult)> = check
    .sub_check_results_in_id_order()
    .into_iter()
    .map(|sub_check_result| (sub_check_result.id.clone(), sub_check_result.view()))
    .collect();

let page = crate::state::page(
    all,
    input.max_results,
    input.next_token.as_deref(),
    crate::state::Listing::SubCheckResults,
)
```
and the same handler refusing to answer a request with a quietly empty list when the id resolves to nothing — list_sub_check_results.rs:168-179 (`no_configuration_check_operation_message`).

Test coverage: tests/all/sub_check_result_cursor.rs pins that a token resumed at any `MaxResults` reports every result of the SAME operation exactly once ("a walk that changes its page size between calls still reports every stored result exactly once in `SubCheckResultId` order"); no test resumes a token under a different `OperationId`, which is the case this gap lives in.

## Steps to Reproduce

1. RegisterApplication an application (HANA, with Sid, SapInstanceNumber and credentials) and wait out the registration transition.
2. StartConfigurationChecks with ConfigurationCheckIds=["SAP_CHECK_01"], note OperationId A; after it settles, StartConfigurationChecks with ConfigurationCheckIds=["SAP_CHECK_02"], note OperationId B.
3. Make any later call so both runs settle (each produces 2 sub-check results).
4. ListSubCheckResults(OperationId=A, MaxResults=1) -> one result plus NextToken T (T is hex of "ssmsap:sub-check-results:v1:<a SubCheckResultId of run A>").
5. ListSubCheckResults(OperationId=B, NextToken=T) -> the token is accepted; run B's results whose SubCheckResultId sorts below A's key are silently omitted (often an empty page with no NextToken), instead of being refused as a token this scope never issued.
6. Same with ListOperationEvents: take a NextToken from a 3-milestone operation and replay it on a 1-milestone operation -> empty page, no error.
