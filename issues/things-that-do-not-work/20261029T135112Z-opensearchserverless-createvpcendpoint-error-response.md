Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 69ff0d5dd7a30b3ec79ed56649baca6290c20cfe

# The subnet-list refusal renders the caller's whole (unbounded) id list where the crate's own boundary clause renders '<n> items'

_Filed: 2026-10-29T13:51:12Z_
_Operation: CreateVpcEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `69ff0d5dd7a30b3ec79ed56649baca6290c20cfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateVpcEndpoint` refuses a `subnetIds` list longer than the emulator's own bound by building the refusal through `crate::state::validation_error_message`, passing `crate::state::rendered_id_list(&input.subnet_ids)` as the `Value '<value>'` half of the sentence. `rendered_id_list` joins EVERY entry of the caller's list into the message.

Two things are wrong with that, and both are provable from inside this crate:

1. It breaks the contract `validation_error_message` states for itself. Its doc says the sentence is "the framework renders for a modelled constraint failure" and "One wording, so a refusal a handler makes reads exactly as the identical one made at the boundary does." But this crate's own suite pins what the framework renders for a LIST-length violation: the value half is the item COUNT (`Value '0 items' at 'names'`), not the list's elements. So the handler's clause does not read like the boundary's identical clause — it reads like a scalar member's clause with a list pasted into it.

2. The echo is UNBOUNDED on input the model allows. `SubnetIds` is `@length(min: 1)` with NO maximum (`smithy_query(query="shape", shape="SubnetIds")` -> `{"length": {"min": 1}}`), so a caller may legally send an arbitrarily long list of pattern-valid `subnet-…` ids. Every one of them past the 7th is joined into the `ValidationException` message, so a single refused request produces a response body proportional to the request's list — on the very path whose comment claims the bound is applied "BEFORE anything is allocated". The crate states the no-unbounded-echo rule for itself elsewhere (`MAX_AUDIT_FIELD`), and the one other over-long-list refusal in this crate (`resource_filtered`) deliberately reports the bound and the COUNT rather than the caller's entries.

The defect lives in the shared helper pair (`validation_error_message` + `rendered_id_list`), so the same clause is produced on `UpdateVpcEndpoint`'s four add/remove list refusals and its two result-bound refusals; `CreateVpcEndpoint` is the first handler that calls it. Note that `tests/all/vpc_endpoint_update.rs` currently pins the joined-list spelling for `addSubnetIds` with the stated reason that it matches the boundary validator — that stated reason is the thing the crate's other suite contradicts, so the pin moves with the fix.

## Evidence

The handler, `services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_vpc_endpoint.rs:166-180`:
```
        // `SubnetIds` carries no maximum, so the list this request names is held to the
        // bound that keeps an endpoint's subnet list finite BEFORE anything is allocated or
        // the shared store is locked.
        if input.subnet_ids.len() > crate::state::MAX_SUBNET_IDS {
            return Err(CreateVpcEndpointErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::validation_error_message(
                        "subnetIds",
                        &crate::state::rendered_id_list(&input.subnet_ids),
                        &crate::state::max_length_constraint(crate::state::MAX_SUBNET_IDS),
                    )),
```

The contract it breaks, `src/state.rs:2192-2202`:
```
/// A refusal worded as this service's validation layer words one: the
/// `<n> validation error detected: Value '<value>' at '<member>' failed to satisfy
/// constraint: <constraint>` sentence the framework renders for a modelled constraint
/// failure, for the single violation a handler refuses. One wording, so a refusal a handler
/// makes reads exactly as the identical one made at the boundary does.
pub fn validation_error_message(member: &str, value: &str, constraint: &str) -> String {
```

What the value half actually is for a list member — the crate's own pin of the framework's clause, `tests/all/reported_pattern.rs:249-255`:
```
        (
            "an empty names list",
            "BatchGetCollection",
            json!({ "names": [] }),
            "Value '0 items' at 'names' failed to satisfy constraint: Member must have length \
             greater than or equal to 1",
        ),
```
…asserted as `format!("1 validation error detected: {clause}")` at `tests/all/reported_pattern.rs:280-284` with the stated rule `"case {case:?}: the framework's own clause is kept"`.

What the handler emits instead, `src/state.rs:2204-2207`:
```
/// A list of ids as a refusal names the value that failed it.
pub fn rendered_id_list(entries: &[String]) -> String {
    format!("[{}]", entries.join(", "))
}
```

That the input is unbounded: `smithy_query(query="shape", shape="SubnetIds")` -> `{"type": "list", "length": {"min": 1}, "member": {"target": "SubnetId", "length": {"min": 1, "max": 32}, "pattern": "^subnet-([0-9a-f]{8}|[0-9a-f]{17})$"}}` — no maximum, so the list length is the caller's to choose, and `src/ops/create_vpc_endpoint.rs:166` says so in words: "`SubnetIds` carries no maximum".

The sibling over-long-list refusal that does NOT echo the caller's entries, `src/state.rs:1378-1383`:
```
    if filter.len() > MAX_RESOURCE_FILTER_ENTRIES {
        return Err(format!(
            "resource must name at most {MAX_RESOURCE_FILTER_ENTRIES} resources, got {}",
            filter.len()
        ));
    }
```

The crate's own statement of the no-unbounded-echo rule, `src/state.rs:3214-3218`:
```
/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a policy name are all caller-controlled, so
/// an unbounded echo would let one request emit an arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 256;
```

Test coverage of the behaviour today: `tests/all/vpc_endpoint_update.rs:594-611` pins the joined-list spelling for `addSubnetIds`, with the stated reason "A refusal a handler makes is worded exactly as the identical one the boundary validator makes — the sentence the suites beside this one pin for a modelled constraint failure (see `client_error_status.rs`)"; `tests/all/client_error_status.rs:470-499` only ever pins SCALAR members (`Value 'ab' at 'name'`, `Value null at 'name'`), so nothing there supports rendering a list's elements, and `reported_pattern.rs:253` shows the list case rendering `'0 items'`.

## Steps to Reproduce

1. Send `CreateVpcEndpoint` with `{"name":"my-endpoint","vpcId":"vpc-0123456789abcdef0","subnetIds":[ 7 distinct valid "subnet-xxxxxxxx" ids ]}` — one past `crate::state::MAX_SUBNET_IDS`.
2. Observe the 400 `ValidationException` message: `1 validation error detected: Value '[subnet-…, subnet-…, …all seven…]' at 'subnetIds' failed to satisfy constraint: Member must have length less than or equal to 6`.
3. Compare with the clause this crate's own suite pins for a list-length violation made at the boundary (`tests/all/reported_pattern.rs:249-255`): `Value '0 items' at 'names' failed to satisfy constraint: Member must have length greater than or equal to 1` — the value half is the item count, not the elements. The two readings differ, although `validation_error_message`'s doc (`src/state.rs:2195-2196`) says they must not.
4. Repeat the first call with 100 000 distinct valid `subnet-` ids (the model puts no maximum on `SubnetIds`). The refused request's response body now carries all 100 000 ids, because `rendered_id_list` joins the whole list into the message.
5. Do the same against `UpdateVpcEndpoint` with `{"id":<an ACTIVE endpoint>,"addSubnetIds":[ …same long list… ]}` (`src/ops/update_vpc_endpoint.rs:192-205`) to see the shared helper produce the identical unbounded clause on the update path.
