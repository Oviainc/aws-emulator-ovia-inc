Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 74ee1f66bcc53d984fedaa365b301131ee839c95

# Factor the duplicated endpoint-listing cursor/page walk shared by ListEndpoints and ListSharedEndpoints

_Filed: 2026-10-25T05:01:04Z_
_Operation: ListEndpoints | Class: maintainability_

> _Note: this issue was identified against the code base at commit `74ee1f66bcc53d984fedaa365b301131ee839c95`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two endpoint listings implement the same pagination RULE twice, line for line: (1) decode `NextToken` through `crate::state::decode_endpoint_cursor` and answer `ValidationException` when it is not a token this service issued, and (2) walk `store.endpoints` in key order skipping keys before the cursor, stop when the accumulated page reaches `page_size`, and set `NextToken` from `crate::state::encode_endpoint_cursor` on the first unserved key. The only differences between the two copies are the per-item predicate (owner-account vs. Outpost id) and the operation's own error enum.

The crate already factors every other shared rule of this pair into `src/state.rs` (`page_size`, `settle_all`, `encode_endpoint_cursor`, `decode_endpoint_cursor`, `not_found`), so the cursor/page walk is the one rule left copied into both handlers, and the two copies can drift. They have already drifted in ordering: `ListEndpoints` calls `settle_all` immediately after taking the lock (list_endpoints.rs:177-178) while `ListSharedEndpoints` calls it only after the Outpost resolution (list_shared_endpoints.rs:179-188), and the filter predicate in one copy was changed without the other. A single helper in `state.rs` parameterised by the predicate would keep both responses byte-identical and both existing pagination tests unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_endpoints.rs:156-171:
```
        let resume = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_endpoint_cursor(token) {
                Some(key) => Some(key),
                None => {
                    return Err(ListEndpointsErrorModeled::ValidationException(
```
and list_endpoints.rs:192-199:
```
            if resume.as_ref().is_some_and(|from| key < from) {
                continue;
            }
            if endpoints.len() == page_size {
                next_token = Some(crate::state::encode_endpoint_cursor(key));
                break;
            }
            endpoints.push(endpoint.to_endpoint(&region));
```

Copy 2 — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_shared_endpoints.rs:158-174:
```
        let resume = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_endpoint_cursor(token) {
                Some(key) => Some(key),
                None => {
                    return Err(ListSharedEndpointsErrorModeled::ValidationException(
```
and list_shared_endpoints.rs:205-212 (byte-identical to the block above):
```
            if resume.as_ref().is_some_and(|from| key < from) {
                continue;
            }
            if endpoints.len() == page_size {
                next_token = Some(crate::state::encode_endpoint_cursor(key));
                break;
            }
            endpoints.push(endpoint.to_endpoint(&region));
```

The crate's existing factoring pattern, src/state.rs:507-523: "/// The cursor an endpoint listing returns to resume at `key` ..." `pub fn encode_endpoint_cursor` / `pub fn decode_endpoint_cursor`, and src/state.rs:466-485 `pub fn page_size` — shared rules already live in one place.

Drift already present: list_endpoints.rs:177-178 `let mut store = state.lock(); store.settle_all(now);` versus list_shared_endpoints.rs:179-188 which locks, returns the 404 for an unknown Outpost, and only then calls `store.settle_all(now)`.

Test coverage that must stay unchanged: tests/all/endpoint_lifecycle.rs::list_endpoints_pages_over_one_stable_order (asserts the paged walk equals the single-page answer and that `["not-a-cursor", "op-0a1b2c3d4e5f60718", "aaaa/bbbb"]` are refused) and tests/all/shared_endpoint_listing.rs::paging_an_outposts_listing_walks_the_same_set_in_the_same_order:235 (asserts "paging must walk every stored endpoint of {shared} once, in order").

## Steps to Reproduce

1. Open services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_endpoints.rs and read lines 156-205
2. Open services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_shared_endpoints.rs and read lines 158-218
3. Observe the cursor-decode block and the resume/page-break/next-token block are the same logic in both files, differing only in the per-item predicate and the error enum
4. Note that src/state.rs already hosts every other rule the pair shares (page_size, settle_all, encode_endpoint_cursor, decode_endpoint_cursor, not_found), and that the two copies have already drifted on where settle_all is called
