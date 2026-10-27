Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# scanNameArn is built from the raw credential-scope region, so it can violate the modelled ScanNameArn shape and become untaggable

_Filed: 2026-10-27T02:55:00Z_
_Operation: CreateScan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateScan` stores and returns `scanNameArn` built by interpolating the request's SigV4 credential-scope region verbatim into the ARN. The region is request-controlled text: `sigv4_credential_scope` only rejects an EMPTY region, so any other value (one containing whitespace, or an arbitrarily long one) flows straight into the ARN.

The crate already recognises this hazard and guards the other place the region is interpolated — `url_region` normalises a scope that is not Region-shaped to the default region before it reaches the upload HOST, and that guard is covered by a test. `scan_name_arn` needs the same guard and does not have it, so:

* the `scanNameArn` reported by `CreateScan`, `GetScan` and `ListScans` can carry a value its own member's shape forbids — `ScanNameArn` is `@length 1..300` with `@pattern ^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$`, so a region containing a space breaks the `[\S]+` class and a long region breaks the 300-character bound. The crate states the opposite rule for the other constrained member it reports: `reportable_scan_name` exists "so a response never carries a value its own member's shape forbids".
* the scan then cannot be addressed by the ARN the service handed back: the tagging operations resolve a `resourceArn` through `parse_scan_arn`, whose `is_arn_region` refuses any region containing whitespace, and a too-long or whitespace-bearing `resourceArn` is refused at the request boundary before the handler runs. `TagResource`, `UntagResource` and `ListTagsForResource` are therefore permanently unusable for such a scan.

## Evidence

Unguarded use — `src/ops/create_scan.rs:221` (`let region = crate::state::request_region(&ctx.parts().headers);`) and `src/ops/create_scan.rs:473`:
```
                        scan_name_arn: crate::state::scan_name_arn(&region, &input.scan_name),
```
`src/state.rs:1040-1051`:
```
/// The region this request is for, read from its SigV4 credential scope.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
...
pub fn scan_name_arn(region: &str, scan_name: &str) -> String {
    ::engine::emulator_arn(SERVICE_CODE, region, &format!("scans/{scan_name}"))
}
```
The guard this path skips, applied on the sibling host path — `src/state.rs:1149-1161`:
```
/// The region a published URL's host names: the request's own when it has a Region's
/// shape, else [`DEFAULT_REGION`].
///
/// A credential scope is request-controlled text, and this value is interpolated into a
/// host, so only a value shaped like a Region may reach it ...
fn url_region(region: &str) -> &str {
    if is_region_shaped(region) { region } else { DEFAULT_REGION }
}
```
The crate's own rule for a constrained reported member — `src/state.rs:1299-1304`: "... so a response never carries a value its own member's shape forbids."
The parser the tagging operations go through — `src/state.rs:1251-1254`:
```
/// `[\S]+`: one or more non-whitespace characters.
fn is_arn_region(region: &str) -> bool {
    !region.is_empty() && !region.chars().any(char::is_whitespace)
}
```
used by `src/state.rs:895-900` (`scan_by_arn` -> `parse_scan_arn(arn)?`), which `src/ops/tag_resource.rs:188`, `src/ops/untag_resource.rs:185` and `src/ops/list_tags_for_resource.rs:177` all resolve through.
No sanitisation downstream — `vendor/server/src/lib.rs:162-164`: `format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")`; `vendor/server/src/lib.rs:1800` rejects only an empty region.
Model: `ScanNameArn` is `len 1..300, pattern ^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$` (`OPERATIONS.md:37`).
Test coverage: `tests/all/upload_target.rs:704-732` drives scopes `"NOT_A_REGION"` and `"evil.example.com"` and asserts the published host falls back to `DEFAULT_REGION`; no test drives a non-Region-shaped scope through `CreateScan`/the tagging operations.

## Steps to Reproduce

1. Call `CreateUploadUrl` and then `CreateScan(scanName="demo", resourceId.codeArtifactId=<minted>)` with `authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us east-1/codeguru-security/aws4_request, ...` (a credential scope whose region segment contains a space).
2. Read the response's `scanNameArn`: `arn:aws:codeguru-security:us east-1:000000000000:scans/demo` — the `[\S]+` region class of the modelled `ScanNameArn` pattern does not admit the space.
3. Call `GET /tags/<that ARN>` (ListTagsForResource) or `POST /tags/<that ARN>` (TagResource) with the ARN the service just returned: the request is refused (malformed `resourceArn`), so the scan can never be tagged through its own ARN.
4. Repeat with a credential scope whose region segment is a few hundred characters long: the returned `scanNameArn` exceeds the shape's `@length` maximum of 300.
