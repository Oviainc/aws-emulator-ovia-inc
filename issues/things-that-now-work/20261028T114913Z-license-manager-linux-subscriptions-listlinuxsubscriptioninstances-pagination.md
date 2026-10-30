Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The minted NextToken embeds every filter value verbatim, so a model-allowed Filters selection yields a cursor longer than the modelled NextToken maximum

_Filed: 2026-10-28T11:49:13Z_
_Operation: ListLinuxSubscriptionInstances | Class: pagination_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination cursor these lists mint is `hex("<kind>:<offset>")`, where `<kind>` embeds the FULL filter selection rendered by `crate::state::filter_signature` — every `Filter$Name`, `Filter$Operator` and every `Filter$Values` entry, length-prefixed and verbatim. Hex doubles that, so the token's length is roughly twice the total size of the request's filter values.

The model bounds the `NextToken` a client may SEND back at 16384 characters, and `Filter$Values` is a `StringList` of up to 100 entries of up to 100 characters each, with `FilterList` itself carrying no length bound at all. A single model-allowed filter with 100 maximal values therefore produces a signature of ~10,400 characters and a token of ~20,900 characters — over the modelled input maximum. The first page answers 200 with that token, and the continuation request carrying it is refused at the request boundary as a `ValidationException` (the `@length` envelope `src/service/validation.rs` words), so a walk that the service itself started cannot be finished: the client gets a hard refusal for a cursor the service handed it, instead of the next page.

The same helper and the same cursor shape are used by `ListLinuxSubscriptions` (`crate::state::filter_signature` + `crate::state::paginate`), so the defect is shared; `ListRegisteredSubscriptionProviders` is unaffected in practice because its selection is an enum list. No comment, test or doc in the crate acknowledges the 16384 bound.

## Evidence

The modelled input bound (smithy_query(query="operation",operation="ListLinuxSubscriptionInstances")):
```
"NextToken": {"target": "String", "length": {"min": 1, "max": 16384}}
```
and the size the filter selection may reach (smithy_query(query="shape",shape="FilterList,StringList")):
```
"FilterList": {"type": "list", "member": "Filter"}            # no @length at all
"StringList": {"type": "list", "length": {"min": 1, "max": 100}, "member": {"target": "String", "length": {"min": 1, "max": 100}}}
```
The cursor the handler asks for (src/ops/list_linux_subscription_instances.rs:195-206):
```
        let (page, next_token) = crate::state::paginate(
            &selected,
            &format!(
                "linux-subscription-instances/{}",
                crate::state::filter_signature(&filters)
            ),
```
The signature, which copies every value in (src/state.rs:910-924):
```
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        signature.push_str(&length_prefixed(
            [filter.name.clone().unwrap_or_default(), filter.operator.clone().unwrap_or_default()]
                .into_iter()
                .chain(filter.values.clone().unwrap_or_default()),
        ));
```
and the hex doubling (src/state.rs:875-877):
```
fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}
```
Test coverage: `tests/all/linux_subscriptions.rs:24-26` pins only that `MaxResults`/`NextToken` page each list once and that a cursor minted for one selection is not honoured for another — every fixture filter carries a handful of short values, so no test mints a cursor anywhere near the 16384 bound.

## Steps to Reproduce

1. Seed or discover at least three instances so a page can be truncated (as tests/all/linux_subscriptions.rs::discover does).
2. POST /subscription/ListLinuxSubscriptionInstances with {"MaxResults":1,"Filters":[{"Name":"InstanceID","Operator":"NotEqual","Values":[<100 distinct 100-character strings>]}]} — every value is inside the modelled `StringList` bounds (len 1..100 entries, each 1..100 chars).
3. Read `NextToken` from the 200 response and note its length: ~20,900 characters, because it is hex("linux-subscription-instances/<~10,400-char signature>:1").
4. POST the same request again with that `NextToken` added.
5. Observe a 400 ValidationException from the request boundary for a `NextToken` longer than the modelled maximum of 16384 (smithy model: ListLinuxSubscriptionInstancesRequest$NextToken @length 1..16384), so the walk the service started cannot be continued.
6. Repeat against /subscription/ListLinuxSubscriptions with {"Filters":[{"Name":"Subscription",...}]} for the same result from the shared helper.
