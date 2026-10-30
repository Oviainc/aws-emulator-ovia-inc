Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0559e2588b1548e68d881f6a6f917eedea630bd1

# Feed and dictionary ARNs are minted from the raw credential-scope region, so a created resource can carry an ARN this crate's own tagging route refuses as malformed

_Filed: 2026-10-28T18:55:09Z_
_Operation: CreateFeed | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0559e2588b1548e68d881f6a6f917eedea630bd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateFeed` and `CreateDictionary` build the resource's stored `arn` by interpolating `crate::state::request_region` verbatim. `request_region` returns the SigV4 credential scope's region exactly as the caller wrote it in the `Authorization` header: the producer in the framework only rejects an EMPTY field, so `US-EAST-1`, `us_east_1`, `Local`, or any other non-empty string is passed straight through.

The modelled `ResourceArn` — the ONLY handle the three tagging operations take — restricts the region segment to `[a-z0-9\-]+`. That `@pattern` is enforced by the centralized request walk before any handler runs (this crate's tests pin exactly that pre-handler refusal for an `@httpLabel` member). So when a request is signed for a credential scope whose region carries any character outside that class:

* `CreateFeed` answers 202 with `arn: arn:aws:elemental-inference:US-EAST-1:000000000000:feed/<id>` and persists it as `Feed::arn`; `CreateDictionary` does the same for `Dictionary::arn`;
* `TagResource`, `UntagResource` and `ListTagsForResource` for that very ARN are then refused with HTTP 400 `ValidationException` before `tagged_resource`/`tagged_resource_mut` is ever reached, because the only string that resolves the record (both resolvers require `feed.arn == resource_arn` / `dictionary.arn == resource_arn`) is a string the boundary rejects;
* the resource is still fully reachable by id (`GetFeed`, `ListFeeds`, `UpdateFeed`, `DeleteFeed`, `GetDictionary`, …), so it exists but is PERMANENTLY untaggable and its tag set can never be listed.

The two sides contradict each other inside this one crate: the create door writes an ARN the tagging door is built to reject, while `state.rs` claims the minted form is the one the modelled pattern admits and that the id-based and ARN-based paths "cannot disagree about which resources a Region holds".

## Evidence

The region is taken verbatim from the caller's header — `services/service-com-amazonaws-elementalinference-elementalinference/src/state.rs:1919-1925`:
```
/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
and its producer applies no character rule, only non-emptiness — `vendor/server/src/lib.rs:1800-1803`:
```
    if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() {
        return None;
    }
    Some(SigV4Scope {
```

It is interpolated straight into the stored ARN — `src/state.rs:1742-1745`:
```
/// The ARN of a feed in `region`: `arn:aws:elemental-inference:<region>:<account>:feed/<id>`.
pub fn feed_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("feed/{id}"))
}
```
over `vendor/server/src/lib.rs:162-164`:
```
pub fn emulator_arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")
}
```
and stored as the record's only ARN handle — `src/ops/create_feed.rs:378-379`: `let feed = crate::state::Feed { arn: crate::state::feed_arn(&region, &id),` and `src/ops/create_dictionary.rs:272-273`: `let dictionary = crate::state::Dictionary { arn: crate::state::dictionary_arn(&region, &id),`.

What the model allows that string to be — `smithy_query(query="operation",operation="TagResource,ListTagsForResource")`: `"resourceArn": {"target": "ResourceArn", "httpLabel": true, "required": true, "pattern": "^arn:aws[a-z\\-]*:elemental-inference[a-z\\-]*:[a-z0-9\\-]+:[0-9]{12}:(feed|dictionary)/[a-zA-Z0-9]{1,19}$"}` — the region segment is `[a-z0-9\-]+`, so `US-EAST-1` or `us_east_1` cannot appear in a `ResourceArn` (same pattern on `UntagResource$resourceArn`, `OPERATIONS.md:37,53,61`).

The crate's own doc says the minted value IS that form — `src/state.rs:1790-1795`:
```
/// The ARN of a dictionary in `region`:
/// `arn:aws:elemental-inference:<region>:<account>:dictionary/<id>`, the form the
/// modelled `resourceArn` pattern `…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` admits.
pub fn dictionary_arn(region: &str, id: &str) -> String {
```
and that the two routes cannot disagree — `src/state.rs:41-45`: "The ARN-based tagging path resolves within that same Region ([`…::tagged_resource_mut`]), so the id-based and the ARN-based paths cannot disagree about which resources a Region holds.", plus `src/state.rs:1304-1305`: "One resolution shared by every operation that reaches a resource by its `resourceArn`, so none of them can disagree about what an ARN names."

The ARN resolvers can only ever match the stored string — `src/state.rs:1318-1325`: `"feed" => self.feed_mut(region, id).filter(|feed| feed.arn == resource_arn)` / `"dictionary" => self.dictionary_mut(region, id).filter(|dictionary| dictionary.arn == resource_arn)`.

That a `@pattern` on an `@httpLabel` member is refused pre-handler is pinned — `tests/all/error_contract.rs:326-338` (`AssociateFeed.id, bound to the URI label`, `/v1/feed/NOT-A-FEED-ID/associate` → 400 `ValidationException` naming the pattern) and `:361-387`.

Test coverage: every suite signs for a canonical lowercase region, so no test exercises a credential-scope region outside `[a-z0-9\-]` — `tests/all/harness.rs:24-26`: "The region every request below is signed for. The service reads it out of the SigV4 credential scope, so the ARNs it mints must carry exactly this region." `pub const REGION: &str = "us-west-2";`, `tests/all/region_isolation.rs:36`: `pub const REGIONS: [&str; 4] = ["us-west-2", "us-east-1", "eu-central-1", "ap-southeast-2"];`, and `tests/all/harness.rs:174` builds the header as `Credential=AKIDEXAMPLE/20240101/{region}/{SIGNING_NAME}/aws4_request`.

## Steps to Reproduce

1. POST /v1/feed with header `Authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/US-EAST-1/elemental-inference/aws4_request, SignedHeaders=host, Signature=x` and body {"name":"arnfeed","outputs":[{"name":"crop1","outputConfig":{"cropping":{}},"status":"ENABLED"}],"tags":{"env":"dev"}} — observe HTTP 202 and `arn` = `arn:aws:elemental-inference:US-EAST-1:000000000000:feed/<id>`
2. Compare that value with the modelled resourceArn pattern `^arn:aws[a-z\-]*:elemental-inference[a-z\-]*:[a-z0-9\-]+:[0-9]{12}:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` — the uppercase region segment does not satisfy it, so the create emitted an ARN the model says a resourceArn cannot hold
3. GET /v1/tags/{that ARN, percent-encoded} signed for the same US-EAST-1 scope. Expected: HTTP 200 reporting the feed's `env=dev` tag (the ARN is the one this service just handed out). Actual: HTTP 400 ValidationException naming the resourceArn pattern — the refusal tests/all/error_contract.rs:326-338 pins for a label that violates its pattern, raised before the handler runs
4. POST /v1/tags/{same ARN} with {"tags":{"added":"yes"}} and DELETE /v1/tags/{same ARN}?tagKeys=env — both HTTP 400 for the same reason, so the feed can never be tagged, untagged, or have its tags listed
5. GET /v1/feed/{id} signed for the same scope — HTTP 200, proving the feed exists and is reachable by id: only the ARN-addressed operations are permanently unreachable
6. Repeat steps 1-5 with POST /v1/dictionary ({"name":"arndict","language":"eng"}) — the same break through `dictionary_arn`, whose own doc claims the minted form is the one the modelled pattern admits
