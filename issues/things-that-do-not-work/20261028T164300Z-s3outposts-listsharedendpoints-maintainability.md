Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0b60124a277040298c4e92ee09b21d27ce53d94f

# The endpoint-listing paging test harness is implemented twice (and the two copies have already drifted)

_Filed: 2026-10-28T16:43:00Z_
_Operation: ListSharedEndpoints | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0b60124a277040298c4e92ee09b21d27ce53d94f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate's test tree carries two independent copies of the same listing-paging harness: `tests/all/endpoint_paging.rs` and `tests/all/one_listing_rule.rs`. Both define a `Listing` enum with an `operation()` name map, both build the listing request query (`outpostId`/`maxResults`/`nextToken` joined with `&`, prefixed with `?`) and dispatch to `list_endpoints`/`list_shared`, both define a byte-for-byte identical `create_endpoints` fixture builder (same `subnet-{:05x}{:012x}` / `sg-{:05x}{:012x}` id formula, same assertion message), and both build the same fixture (3 endpoints on `owned[0]`, 2 on `owned[1]`, 2 on `shared`). `reported_arns` and `stored_arns` are likewise re-implemented across `endpoint_paging.rs`, `endpoint_teardown.rs` and `shared_endpoint_listing.rs`.

The copies have ALREADY drifted, which is the concrete harm: the "a token this service never issued is refused" tables differ between the two files — `endpoint_paging.rs` has 8 rows, `one_listing_rule.rs` has 6, and each carries rows the other lacks. So a change to the cursor form has to be made in two places and a reviewer cannot tell from either file which table is authoritative.

The crate already has the single place this harness belongs: `tests/all/endpoint_lifecycle.rs` is the module both files import `create`, `list_endpoints`, `list_shared`, `owned_outposts`, `shared_outpost`, `string_at` and `Answer` from. Moving the `Listing`/query-builder/`create_endpoints`/fixture/`reported_arns`/`stored_arns` helpers there (keeping the union of the refusal rows) leaves every handler response and every existing test assertion unchanged — it only removes the second copy.

## Evidence

Copy 1 — `services/service-com-amazonaws-s3outposts-s3outposts/tests/all/endpoint_paging.rs:121`:
```rust
async fn create_endpoints(router: &MultiServiceRouter, outpost: &str, count: usize, tag: usize) {
    for index in 0..count {
        let request = json!({
            "OutpostId": outpost,
            "SubnetId": format!("subnet-{:05x}{:012x}", tag, index),
            "SecurityGroupId": format!("sg-{:05x}{:012x}", tag, index),
        });
        let answer = create(router, request).await;
        assert_eq!(
            answer.status, 200,
            "creating endpoint {index} on {outpost} must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
```

Copy 2 — `services/service-com-amazonaws-s3outposts-s3outposts/tests/all/one_listing_rule.rs:169`:
```rust
async fn create_endpoints(router: &MultiServiceRouter, outpost: &str, count: usize, tag: usize) {
    for index in 0..count {
        let answer = create(
            router,
            json!({
                "OutpostId": outpost,
                "SubnetId": format!("subnet-{:05x}{:012x}", tag, index),
                "SecurityGroupId": format!("sg-{:05x}{:012x}", tag, index),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "creating endpoint {index} on {outpost} must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
```

Same `Listing` abstraction twice — `endpoint_paging.rs:30`:
```rust
enum Listing {
    Own,
    OnOutpost(String),
}
...
    fn operation(&self) -> &'static str {
        match self {
            Listing::Own => "ListEndpoints",
            Listing::OnOutpost(_) => "ListSharedEndpoints",
```
versus `one_listing_rule.rs:32`:
```rust
enum Listing {
    Outposts,
    OwnEndpoints,
    EndpointsOn(String),
}
...
    fn operation(&self) -> &'static str {
        match self {
            Listing::Outposts => "ListOutpostsWithS3",
            Listing::OwnEndpoints => "ListEndpoints",
            Listing::EndpointsOn(_) => "ListSharedEndpoints",
```

Same query builder twice — `endpoint_paging.rs:54` and `one_listing_rule.rs:58`, both:
```rust
    if let Some(size) = page_size {
        params.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        params.push(format!("nextToken={token}"));
    }
    let query = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
```

Same fixture twice — `endpoint_paging.rs:145` (`paged_fixture`) and `one_listing_rule.rs:195` (`fixture`), both:
```rust
    let (router, engine) = emulator_with_engine();
    let owned = owned_outposts(&router).await;
    let shared = shared_outpost(&router).await;
    create_endpoints(&router, &owned[0], 3, 0x1).await;
    create_endpoints(&router, &owned[1], 2, 0x2).await;
    create_endpoints(&router, &shared, 2, 0x3).await;
```

The copies have drifted — `endpoint_paging.rs:253` lists 8 refused tokens including rows the sibling lacks:
```rust
        ("only the endpoint id", "0000000000000000001".to_string()),
        ("neither part of a published form", "aaaa/bbbb".to_string()),
```
while `one_listing_rule.rs:140` lists 6 and carries a row the other lacks:
```rust
                ("the other listing's Outpost cursor", "op-0a1b2c3d4e5f60718"),
```

`reported_arns` is a third duplicated helper — identical bodies at `endpoint_paging.rs:99` and `endpoint_teardown.rs:58`:
```rust
fn reported_arns(answer: &Answer) -> Vec<String> {
    answer
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect()
}
```

The single home already exists and is already imported by both files — `one_listing_rule.rs:25`:
```rust
use crate::endpoint_lifecycle::{
    create, emulator_with_engine, endpoint_id_of, list_endpoints, list_outposts, list_shared,
    owned_outposts, shared_outpost, string_at, Answer,
};
```

Tests that cover the duplicated logic and must keep passing unchanged: `endpoint_paging.rs::both_endpoint_listings_page_over_one_stable_order`, `::both_endpoint_listings_refuse_a_token_they_never_issued`, `::both_endpoint_listings_refuse_a_page_size_past_the_published_ceiling`, `one_listing_rule.rs::every_listing_pages_over_one_stable_order`, `::every_listing_resumes_at_the_key_its_cursor_names`, `::every_listing_refuses_a_token_it_never_issued`, plus `endpoint_teardown.rs::a_deleted_endpoint_reports_deleting_until_the_clock_completes_the_teardown`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-s3outposts-s3outposts/tests/all/endpoint_paging.rs and services/service-com-amazonaws-s3outposts-s3outposts/tests/all/one_listing_rule.rs side by side.
2. Compare `create_endpoints` (endpoint_paging.rs:121 vs one_listing_rule.rs:169): the two bodies are the same computation, down to the subnet/security-group id formula and the assertion message.
3. Compare the `Listing` enum and its `operation()` map (endpoint_paging.rs:30 vs one_listing_rule.rs:32) and the request-query builder (endpoint_paging.rs:48 vs one_listing_rule.rs:52): the second file's `Listing` is a superset of the first's, with the same dispatch logic restated.
4. Compare the fixture builders (endpoint_paging.rs:140 `paged_fixture` vs one_listing_rule.rs:190 `fixture`): identical bodies apart from the `listings` vector they return.
5. Compare the never-issued-token tables (endpoint_paging.rs:253 vs one_listing_rule.rs:127): 8 rows versus 6, each with rows the other lacks — the two copies of the same rule have already drifted.
6. Compare `reported_arns` at endpoint_paging.rs:99 with endpoint_teardown.rs:58: the same helper written twice.
7. Note that both files already import the crate's shared test-helper module `crate::endpoint_lifecycle` (one_listing_rule.rs:25), so the single place for this harness exists and is unused for it.
