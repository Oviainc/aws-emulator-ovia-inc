Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# CreateOriginEndpoint stores Whitelist entries, CDN Authorization ARNs and an empty CMAF manifest list without any validation

_Filed: 2026-10-24T07:15:59Z_
_Operation: CreateOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: MediaPackage validates the contents of an OriginEndpoint's access-control and packaging members before it stores the endpoint, refusing anything it could not act on with the modelled `UnprocessableEntityException` (HTTP 422) and creating nothing:

1. `Whitelist` is "a list of source IP CIDR blocks that will be allowed to access the OriginEndpoint". An entry that is not a CIDR block (`not-a-cidr`, `10.0.0.1` with no prefix length, `10.0.0.0/33`, `999.1.1.1/8`) is not an address range the egress tier could ever match, so AWS refuses it rather than storing it as an access-control rule that silently matches nothing.
2. `Authorization` is "CDN Authorization credentials" and requires `CdnIdentifierSecret` — "the ARN for the secret in Secrets Manager that your CDN uses for authorization" — and `SecretsRoleArn` — "the ARN for the IAM role that allows MediaPackage to communicate with AWS Secrets Manager". AWS resolves both up front: a value that is not an ARN of the right service/type, or a role MediaPackage cannot assume, is refused, because an endpoint stored with unreadable CDN credentials would reject every playback request.
3. A `CmafPackage` whose `HlsManifests` list is empty (or absent) describes a package that publishes no manifest at all, so there is nothing for the endpoint's playback URL to serve; AWS refuses it instead of storing a package with no child manifests.

Actual: none of the three is checked. `crate::state::bound_origin_endpoint_members` (src/state.rs ~482-526) bounds only the ENTRY COUNT of `whitelist` (<=100) and the CHARACTER COUNT of each entry (<=1024) — any string at all is accepted as a CIDR block and stored; `input.authorization` is cloned straight into `OriginEndpointRecord` by the handler (create_origin_endpoint.rs line ~304, `authorization: input.authorization.clone()`) with no ARN-shape, Secrets Manager or IAM check anywhere in the crate; and `cmafPackage.hlsManifests` is only bounded above (<=50 entries), so `"hlsManifests": []` is accepted and `crate::state::stored_cmaf_package` materialises a package with an empty manifest list. All of it is then reported verbatim by `DescribeOriginEndpoint`/`ListOriginEndpoints` as live configuration.

The same unvalidated members travel the same shared helper on `UpdateOriginEndpoint` (update_origin_endpoint.rs lines ~221-237 call the identical `bound_origin_endpoint_members`), so the fix belongs in that one helper and covers both writes. This issue is NOT about the endpoint `Id` — that cause is already filed under CreateChannel/validation, which names all three creates.

## Evidence

Spec grounding: `smithy_query(query="operation",operation="CreateOriginEndpoint")` documents `Whitelist` as "A list of source IP CIDR blocks that will be allowed to access the OriginEndpoint"; `smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization")` shows both members required and documented as a Secrets Manager secret ARN and an IAM role ARN; the operation declares `UnprocessableEntityException` (httpError 422) as its client-input error. The model publishes no `@pattern` on any of them (they target the bare `__string`), which is exactly why the framework's constraint walk cannot catch this and the handler has to.

Handler: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_origin_endpoint.rs` lines 211-233 (the complete set of pre-store checks: `bound_chars` on ids, `bound_tags`, `bound_origin_endpoint_members`, `bound_origin_endpoint_durations`) and line 304 (`authorization` copied verbatim). `src/state.rs` 482-526 shows `whitelist` and `cmafPackage.hlsManifests` judged by count/length only.

In-repo corroboration: the crate already performs exactly this class of cross-store pre-flight for a role ARN — `crate::state::harvest_role_refusal` (state.rs ~1459) resolves `s3Destination.roleArn` against the IAM store and refuses a role whose trust policy withholds `sts:AssumeRole` — so the seam for validating `SecretsRoleArn` exists and is unused. The sibling open issues `20261024T051129Z-mediapackage-vod-updatepackaginggroup-validation.md` ("stores CDN Authorization ARNs unvalidated, accepting values AWS refuses with 422") and `20261024T010403Z-mediapackage-vod-createpackagingconfiguration-validation.md` ("A package block carrying an EMPTY manifest list is accepted and stored instead of being refused") file the same two causes against the VOD sibling.

Test coverage: `tests/all/channel_lifecycle.rs` exercises only well-formed values — `"whitelist": ["10.0.0.0/8", "192.168.0.0/16"]`, a 100-entry list of valid `/32`s at the count bound (lines ~862-933), and an `authorization` block carrying a real-looking secret/role ARN pair (lines ~544-548). No test sends a malformed CIDR, a non-ARN authorization value, or an empty `hlsManifests` list, so these paths are uncovered rather than pinned.

## Steps to Reproduce

1. Boot the emulator and point boto3 at it; mp.create_channel(Id="c1").
2. mp.create_origin_endpoint(Id="oe-cidr", ChannelId="c1", HlsPackage={}, Whitelist=["not-a-cidr", "10.0.0.0/33", "999.1.1.1/8"]) -> observe HTTP 200 and the junk entries echoed back; AWS refuses with UnprocessableEntityException / 422 and creates nothing.
3. mp.describe_origin_endpoint(Id="oe-cidr") -> the unusable whitelist is reported as live access control.
4. mp.create_origin_endpoint(Id="oe-auth", ChannelId="c1", HlsPackage={}, Authorization={"CdnIdentifierSecret": "not-an-arn", "SecretsRoleArn": "arn:aws:iam::000000000000:role/never-created"}) -> observe HTTP 200; AWS refuses the unreadable CDN credentials with 422.
5. mp.create_origin_endpoint(Id="oe-cmaf", ChannelId="c1", CmafPackage={"HlsManifests": []}) -> observe HTTP 200 and a stored CMAF package with no child manifests; AWS refuses a package that publishes no manifest.
6. Repeat steps 2 and 4 against mp.update_origin_endpoint on an existing endpoint -> the same values are accepted there too.
