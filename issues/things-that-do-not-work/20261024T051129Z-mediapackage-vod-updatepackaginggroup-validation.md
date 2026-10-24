Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# UpdatePackagingGroup stores CDN Authorization ARNs unvalidated, accepting values AWS refuses with 422

_Filed: 2026-10-24T05:11:29Z_
_Operation: UpdatePackagingGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePackagingGroup` is the operation that attaches (or replaces) a packaging group's CDN authorization. Both members of the `Authorization` structure are documented ARNs: `CdnIdentifierSecret` is "The Amazon Resource Name (ARN) for the secret in AWS Secrets Manager that is used for CDN authorization" and `SecretsRoleArn` is "The Amazon Resource Name (ARN) for the IAM role that allows MediaPackage to communicate with AWS Secrets Manager". AWS validates both before persisting the configuration — the service must assume that role and read that secret to enforce CDN authorization at egress — and refuses a value that is not an ARN of the expected service with `UnprocessableEntityException` (HTTP 422, "The parameters sent in the request are not valid."), leaving the group's stored authorization untouched.

Expected: `PUT /packaging_groups/{Id}` with `{"authorization": {"cdnIdentifierSecret": "not-an-arn", "secretsRoleArn": "nope"}}` is refused with UnprocessableEntityException / 422 attributing the violation to the offending member, and a following `DescribePackagingGroup` still reports the authorization the group had before the call (or none).

Actual: the handler resolves the group by (Region, Id) and then assigns the request's block straight into the record — `stored.authorization = input.authorization;` — with no syntax check, no service check and no emptiness check. The call answers HTTP 200, the junk block is echoed back, and every later `DescribePackagingGroup` / `ListPackagingGroups` projection reports a group that claims CDN authorization is configured from values no real deployment could use. Single-character and empty-ish values are accepted identically.

## Evidence

Spec: `smithy_query(query="shape", shape="Authorization")` — `CdnIdentifierSecret` and `SecretsRoleArn` are both `required` and documented as ARNs (Secrets Manager secret, IAM role). `UpdatePackagingGroup` models `UnprocessableEntityException` (errorKind client, httpError 422), which is this surface's refusal channel for invalid request parameters. Rejecting syntactically invalid ARNs for members documented as ARNs is well-established AWS control-plane behavior.
Handler: `src/ops/update_packaging_group.rs` `handle()` — the only check is the 404 for an `Id` the request's Region holds no group under (lines ~181-191); line ~197 is `stored.authorization = input.authorization;`, i.e. the block is persisted verbatim.
Tests: `tests/all/packaging_group_update.rs::authorizations()` currently PINS the lenient behavior — the row `("one-character values", json!({ "cdnIdentifierSecret": "a", "secretsRoleArn": "b" }))` is asserted to answer HTTP 200 and to be reported back by the following describe (and its doc comment notes the members carry "no length or pattern constraint"). A fix must therefore also replace that row with a rejection case; no test asserts any ARN-shape refusal today.
Not a duplicate: the open issue `20261024T010403Z-mediapackage-vod-createasset-validation` files the same class for `CreateAsset`'s `SourceArn`/`SourceRoleArn`, and `20261024T010403Z-mediapackage-vod-createpackaginggroup-validation` covers tag-on-create validation for `CreatePackagingGroup`. Neither touches `UpdatePackagingGroup`, the only operation that can set authorization on an existing group.

## Steps to Reproduce

1. POST /packaging_groups with {"id": "vod-group"} (signed for any region) and observe HTTP 200.
2. PUT /packaging_groups/vod-group with {"authorization": {"cdnIdentifierSecret": "not-an-arn", "secretsRoleArn": "nope"}}: observe HTTP 200 where AWS refuses with UnprocessableEntityException (422) and persists nothing.
3. GET /packaging_groups/vod-group: observe the stored group now reports the unusable authorization block.
4. Repeat with one-character values ("a"/"b") and with ARNs of the wrong services (e.g. an S3 object ARN as cdnIdentifierSecret, a Secrets Manager ARN as secretsRoleArn): observe the same unvalidated acceptance.
