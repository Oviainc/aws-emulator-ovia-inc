Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-identity-basic-identity-and-openid-token-flow
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-identity-basic-identity-and-openid-token-flow cognito-identity-developer-authenticated-identity-flow cognito-identity-developer-authenticated-identity-merge-and-unlink cognito-identity-get-credentials-for-identity-errors cognito-identity-get-identity-pool-roles-rules-configuration-structure-preserved cognito-identity-get-openid-token-lifecycle cognito-identity-identity-pool-lifecycle cognito-identity-identity-pool-roles-workflow cognito-identity-identity-pool-tagging-workflow cognito-identity-multiple-pools-tag-isolation cognito-identity-principal-tag-attribute-mapping cognito-identity-role-mapping-configuration cognito-identity-tag-isolation-across-pools cognito-identity-tag-operations-error-paths cognito-identity-unauthenticated-guest-identity-flow cognito-identity-unlink-identity-login-management cognito-identity-update-pool-put-overwrite-semantics cognito-identity-identity-pool-create-time-tags-persist cognito-identity-identity-pool-lifecycle-crud cogid-login-list-identity-pools-next-token-pagination cognito-identity-principal-tag-attribute-mapping-configuration
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|CreateIdentityPool|2xx->2xx|$.IdentityPoolTags
Issue-Key: cognito-identity|UpdateIdentityPool|2xx->2xx|$.IdentityPoolTags
Issue-Key: cognito-identity|DescribeIdentityPool|2xx->2xx|$.CreationDate
Issue-Key: cognito-identity|DescribeIdentityPool|2xx->2xx|$.IdentityPoolArn
Issue-Key: cognito-identity|DescribeIdentityPool|2xx->2xx|$.IdentityPoolTags
Issue-Key: cognito-identity|DescribeIdentityPool|2xx->2xx|$.LastModifiedDate
Issue-Key: cognito-identity|ListIdentityPools|2xx->2xx|$.IdentityPools[*].CreationDate
Issue-Key: cognito-identity|ListIdentityPools|2xx->2xx|$.IdentityPools[*].IdentityPoolArn
Issue-Key: cognito-identity|ListIdentityPools|2xx->2xx|$.IdentityPools[*].LastModifiedDate
Issue-Key: cognito-identity|ListIdentityPools|2xx->2xx|$.IdentityPools[*]
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|2xx->2xx|$.PrincipalTags.client
Issue-Key: cognito-identity|GetPrincipalTagAttributeMap|2xx->2xx|$.PrincipalTags.client
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: identity-pool and principal-tag responses omit modeled members

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdentityPool

## Description
This issue bundles 8 root causes: successful (HTTP 200) responses of the identity-pool and principal-tag-map operations are served with members missing from the envelope. Smithy: the IdentityPool shape (output of CreateIdentityPool, DescribeIdentityPool and UpdateIdentityPool) models IdentityPoolTags as a map, and SetPrincipalTagAttributeMapResponse/GetPrincipalTagAttributeMapResponse model PrincipalTags as a map; CreationDate, LastModifiedDate and IdentityPoolArn are not in the model, so for those three the requirement is recorded behavior only — every recorded CreateIdentityPool/DescribeIdentityPool/ListIdentityPools response carries them. The emulator omits the member entirely where AWS sends it (including sending IdentityPoolTags as an empty object for an untagged pool, rather than dropping it). Why: a client that reads pool.IdentityPoolArn, pool.CreationDate or the returned tag/principal-tag map gets a null — ARNs cannot be constructed, tag round-trips appear empty, and principal-tag mappings read back as unset although they were accepted. Fix: the response serializers/projections for these operations (the pool-state to IdentityPool projection, and the principal-tag-map projection), and the state record if the timestamps are not stored at all.

Cause 1 of 8: CreateIdentityPool for a pool created without tags. AWS: 200 with `IdentityPoolTags: {}` present. Emulator: member absent. Fix: IdentityPool response projection (serialize an empty map, do not skip it). Traces: cognito-identity-basic-identity-and-openid-token-flow#setup#1 (19 steps).

Cause 2 of 8: UpdateIdentityPool. AWS: 200 with `IdentityPoolTags: {}` present. Emulator: absent. Fix: as cause 1. Traces: cognito-identity-identity-pool-lifecycle#task#3 (2 steps).

Cause 3 of 8: DescribeIdentityPool for a pool with create-time tags. AWS: 200 carrying CreationDate, LastModifiedDate (epoch-seconds timestamps), IdentityPoolArn (arn:aws:cognito-identity:<region>:<account>:identitypool/<pool-id>) and IdentityPoolTags. Emulator: all four absent. Fix: as cause 1 + pool state must record creation/modification times. Traces: cognito-identity-identity-pool-create-time-tags-persist#task#2 (5 steps).

Cause 4 of 8: DescribeIdentityPool for a pool with tags set after creation. AWS: 200 carrying CreationDate, LastModifiedDate and IdentityPoolArn. Emulator: all three absent. Fix: as cause 3. Traces: cognito-identity-identity-pool-create-time-tags-persist#task#4 (4 steps).

Cause 5 of 8: ListIdentityPools. AWS: each IdentityPools[] entry carries CreationDate, LastModifiedDate and IdentityPoolArn in addition to IdentityPoolId/IdentityPoolName. Emulator: the entries carry only the id and name. Fix: the ListIdentityPools item projection. Traces: cogid-login-list-identity-pools-next-token-pagination#task#3 (2 steps).

Cause 6 of 8: ListIdentityPools after UpdateIdentityPool. AWS: the listed entry carries IdentityPoolArn, CreationDate and LastModifiedDate. Emulator: entry reduced to {IdentityPoolId, IdentityPoolName}. Fix: as cause 5. Traces: cognito-identity-identity-pool-lifecycle#task#5.

Cause 7 of 8: SetPrincipalTagAttributeMap echo. AWS: 200 echoing the accepted PrincipalTags map including every submitted key (the recorded step asserts the 'client' key mapping to 'aud'). Emulator: that key is missing from the echoed map. Fix: the SetPrincipalTagAttributeMap response projection / the stored map (do not drop keys the request supplied). Traces: cognito-identity-principal-tag-attribute-mapping#task#4 (2 steps).

Cause 8 of 8: GetPrincipalTagAttributeMap read-back. AWS: 200 returning the map as it was set, including the 'client' -> 'aud' entry. Emulator: that entry absent. Fix: as cause 7 (and the state it reads). Traces: cognito-identity-principal-tag-attribute-mapping#task#5 (3 steps).

## Evidence
Replay of simparity-wire-aws-v1, status matched (200) in all eight clusters; body members diverge:
- CreateIdentityPool: $.IdentityPoolTags missing (expected {}) (cognito-identity-basic-identity-and-openid-token-flow#setup#1, 19 steps).
- UpdateIdentityPool: $.IdentityPoolTags missing (expected {}) (cognito-identity-identity-pool-lifecycle#task#3, 2 steps).
- DescribeIdentityPool: $.CreationDate, $.IdentityPoolArn ('arn:aws:cognito-identity:<region>:<account>:identitypool/<pool-id>'), $.IdentityPoolTags, $.LastModifiedDate all missing (cognito-identity-identity-pool-create-time-tags-persist#task#2, 5 steps).
- DescribeIdentityPool: $.CreationDate, $.IdentityPoolArn, $.LastModifiedDate missing (cognito-identity-identity-pool-create-time-tags-persist#task#4, 4 steps).
- ListIdentityPools: $.IdentityPools[0].CreationDate, .IdentityPoolArn, .LastModifiedDate missing (cogid-login-list-identity-pools-next-token-pagination#task#3, 2 steps).
- ListIdentityPools: $.IdentityPools[0] expected {CreationDate, IdentityPoolArn, IdentityPoolId, IdentityPoolName, LastModifiedDate}, got {IdentityPoolId, IdentityPoolName} (cognito-identity-identity-pool-lifecycle#task#5).
- SetPrincipalTagAttributeMap: $.PrincipalTags.client missing (expected 'aud') (cognito-identity-principal-tag-attribute-mapping#task#4, 2 steps).
- GetPrincipalTagAttributeMap: $.PrincipalTags.client missing (expected 'aud') (cognito-identity-principal-tag-attribute-mapping#task#5, 3 steps).
Smithy: IdentityPool.IdentityPoolTags (map), SetPrincipalTagAttributeMapResponse.PrincipalTags / GetPrincipalTagAttributeMapResponse.PrincipalTags (map, @length 0..50). CreationDate/LastModifiedDate/IdentityPoolArn are recorded behavior only (not modeled on IdentityPool).

## Steps to Reproduce
1. Replay cognito-identity-basic-identity-and-openid-token-flow step setup#1: CreateIdentityPool without Tags -> response has no IdentityPoolTags member; the recording carries IdentityPoolTags: {}.
2. Replay cognito-identity-identity-pool-create-time-tags-persist#task#2 and #task#4 (DescribeIdentityPool) -> no CreationDate/LastModifiedDate/IdentityPoolArn.
3. Replay cogid-login-list-identity-pools-next-token-pagination#task#3 and cognito-identity-identity-pool-lifecycle#task#5 (ListIdentityPools) -> entries carry only id and name.
4. Replay cognito-identity-principal-tag-attribute-mapping#task#4 then #task#5 -> the 'client' principal-tag key is absent from both the Set echo and the Get read-back.

## Resolution

Every operation that answers with an identity pool now projects the stored pool once
(`ReportedIdentityPool` / `IdentityPoolShortDescription`, built from
`crate::state::IdentityPool`): the pool's ARN, the instant it was created, the instant it
last changed and its tag set — the tag set as the empty map for a pool with no tags. The
pool record now holds both instants; `UpdateIdentityPool`, `TagResource` and
`UntagResource` move the last-modified one when they really change the stored pool, and no
read moves either. Covered by
`tests/all/identity_pool_response_members.rs`.

Causes 1-6 (the `IdentityPoolTags`, `IdentityPoolArn`, `CreationDate` and
`LastModifiedDate` members of CreateIdentityPool, UpdateIdentityPool,
DescribeIdentityPool and the ListIdentityPools entries) are fixed: the ten Issue-Keys over
those four operations no longer diverge on replay.

Causes 7 and 8 (the `client` -> `aud` principal-tag entry echoed by
SetPrincipalTagAttributeMap and read back by GetPrincipalTagAttributeMap) were already
satisfied by the code before this change — `crate::state::DEFAULT_PRINCIPAL_TAGS` carries
that entry and `tests/all/principal_tag_attribute_map.rs` covers the echo and the
read-back — and both their steps (`cognito-identity-principal-tag-attribute-mapping#task#4`
and `#task#5`) now replay clean. The same fixture still diverges at two LATER steps, which
are NOT keys of this issue and are tracked on their own:
`#task#6` (the 400 ValidationException message for a 51-entry `PrincipalTags` map renders
the map sorted by key instead of in AWS's order) by
`20261031T043446Z-cognito-identity-validation-error-messages-render-constraint-collections.md`,
and `#task#8` (AWS serves a mapping for an `IdentityProviderName` the pool does not
declare, where this emulator refuses it) by
`20261029T230931Z-cognito-identity-vended-tokens-and-credentials-are-not.md`.
