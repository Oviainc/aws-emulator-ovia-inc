Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-policy-store-alias-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-policy-store-alias-workflow
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|CreatePolicyStoreAlias|2xx->2xx|$.aliasArn
Issue-Key: verifiedpermissions|CreatePolicyStoreAlias|2xx->2xx|$.createdAt
Issue-Key: verifiedpermissions|GetPolicyStoreAlias|2xx->2xx|$.aliasArn
Issue-Key: verifiedpermissions|GetPolicyStoreAlias|2xx->2xx|$.createdAt
Issue-Key: verifiedpermissions|GetPolicyStore|4xx->4xx|$.message
Issue-Key: verifiedpermissions|GetPolicyStore|4xx->4xx|$.resourceId
Issue-Key: verifiedpermissions|GetPolicyStore|4xx->4xx|$.resourceType
Issue-Key: verifiedpermissions|GetPolicyStoreAlias|4xx->4xx|$.message
Issue-Key: verifiedpermissions|GetPolicyStoreAlias|4xx->4xx|$.resourceId
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# Verified Permissions policy-store alias ARNs drop the region and alias identifiers are mis-resolved in not-found errors

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyStoreAlias

## Description
Everything the emulator builds around policy-store aliases mishandles the ARN/identifier layer: the alias ARN is assembled with an empty region field, and when an alias-qualified identifier reaches a not-found path the resource-path prefix is never stripped, so the modeled resourceId and resourceType of ResourceNotFoundException describe the wrong resource. This bundle has 4 causes.

Cause 1 of 4: CreatePolicyStoreAlias returns an aliasArn with no region.
AWS: Smithy CreatePolicyStoreAlias output member aliasArn is required and targets ResourceArn, pattern ^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$ — the third colon-separated field is the region, and Verified Permissions is a regional service, so a real alias ARN is arn:aws:verifiedpermissions:<region>:<account>:policy-store-alias/<aliasName>. The recording shows the region populated. The createdAt member is a required date-time timestamp.
Emulator: emits arn:aws:verifiedpermissions::<account>:policy-store-alias/<alias> — the region segment is empty — and renders createdAt as an epoch number (1792644259.6969774) instead of an ISO-8601 string.
Why: the ARN is the identifier a client passes to TagResource/ListTagsForResource and to IAM policy conditions; a region-less ARN does not match any ARN pattern or resource-based policy written for the real service, and ARN-parsing clients read the account id out of the wrong field position only because the segment is present-but-empty.
Fix: the alias ARN construction in the verifiedpermissions crate (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store_alias.rs, or the shared ARN builder it calls) must interpolate the request's signing region; the timestamp half is the shared date-time encoder.
Traces: verifiedpermissions-policy-store-alias-workflow#task#2.

Cause 2 of 4: GetPolicyStoreAlias echoes the same region-less aliasArn.
AWS: GetPolicyStoreAlias output aliasArn is the same required ResourceArn and must round-trip the regional ARN stored at create time; createdAt is again a required date-time.
Emulator: returns the identical region-less arn:aws:verifiedpermissions::<account>:policy-store-alias/<alias> and an epoch-number createdAt, on 2 clustered steps.
Why: reading an alias back gives a client the same unusable ARN, so the defect is not confined to the create response and cannot be worked around by re-reading.
Fix: once the ARN is built correctly at create time and persisted, the read path (src/ops/get_policy_store_alias.rs) serves it; the read path must not re-derive a region-less ARN either.
Traces: verifiedpermissions-policy-store-alias-workflow#task#3.

Cause 3 of 4: GetPolicyStore on an alias-qualified identifier reports the wrong resourceId and resourceType.
AWS: Smithy ResourceNotFoundException (httpError 404) has three required members: message, resourceId ("the unique ID of the resource referenced in the failed request") and resourceType (enum ResourceType, whose values include POLICY_STORE and POLICY_STORE_ALIAS). When the lookup was for an alias, AWS reports resourceType POLICY_STORE_ALIAS and the bare alias name in resourceId — the recording shows resourceId = the alias name with no "policy-store-alias/" prefix.
Emulator: returns 404 (correct status, correct error shape) but with resourceType = "POLICY_STORE" and resourceId = "policy-store-alias/<aliasName>" — the resource-path prefix is carried through verbatim into resourceId and the resource kind is not recognised as an alias.
Why: resourceId/resourceType are the machine-readable half of this error; a client that branches on resourceType to decide whether to create the missing alias or the missing store takes the wrong branch, and one that uses resourceId to retry the lookup retries a prefixed name that can never resolve.
Fix: identifier parsing on the GetPolicyStore lookup path (src/ops/get_policy_store.rs and the shared policy-store identifier resolver) must split a "policy-store-alias/<name>" resource path into its kind and bare name, and the not-found error must be constructed with that kind and name.
Traces: verifiedpermissions-policy-store-alias-workflow#task#10.

Cause 4 of 4: GetPolicyStoreAlias's not-found error reports the prefixed resourceId.
AWS: same ResourceNotFoundException contract — resourceId is the bare alias name.
Emulator: returns 404 with resourceId = "policy-store-alias/<aliasName>" instead of "<aliasName>" (resourceType is correct here).
Why: as in cause 3, the identifier a client reads out of the error is not an identifier it can feed back into the API.
Fix: the alias-name normalisation on the GetPolicyStoreAlias lookup path (src/ops/get_policy_store_alias.rs) must strip the resource-path prefix before both the lookup and the error construction.
Traces: verifiedpermissions-policy-store-alias-workflow#task#12.

Note: the message text of causes 3 and 4 also diverges; that message-rule family is filed separately. The requirements asserted here are the status (404), the error shape, and the values of the required resourceId/resourceType members.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator.

Cause 1 — fixture verifiedpermissions-policy-store-alias-workflow, step task#2, CreatePolicyStoreAlias:
  expected status 200, actual status 200
  $.aliasArn: value_mismatch (expected 'arn:aws:verifiedpermissions:us-west-2:000000000000:policy-store-alias/sim2real-probe-verifiedpermissions-alias-9678df7b', got 'arn:aws:verifiedpermissions::000000000000:policy-store-alias/sim2real-probe-verifiedpermissions-alias-9678df7b')
  $.createdAt: value_mismatch (expected a <date-time> string, got 1792644259.6969774)

Cause 2 — fixture verifiedpermissions-policy-store-alias-workflow, step task#3, GetPolicyStoreAlias (cluster also covers task#9):
  expected status 200, actual status 200
  $.aliasArn: same region-less ARN as cause 1
  $.createdAt: value_mismatch (expected a <date-time> string, got 1792644259.6969774)

Cause 3 — fixture verifiedpermissions-policy-store-alias-workflow, step task#10, GetPolicyStore:
  expected status 404, actual status 404
  $.resourceType: value_mismatch (expected 'POLICY_STORE_ALIAS', got 'POLICY_STORE')
  $.resourceId: value_mismatch (expected 'sim2real-probe-verifiedpermissions-alias-9678df7b', got 'policy-store-alias/sim2real-probe-verifiedpermissions-alias-9678df7b')

Cause 4 — fixture verifiedpermissions-policy-store-alias-workflow, step task#12, GetPolicyStoreAlias:
  expected status 404, actual status 404
  $.resourceId: value_mismatch (expected 'sim2real-probe-verifiedpermissions-alias-9678df7b', got 'policy-store-alias/sim2real-probe-verifiedpermissions-alias-9678df7b')

Model evidence: CreatePolicyStoreAliasOutput.aliasArn / GetPolicyStoreAliasOutput.aliasArn are required, target ResourceArn with pattern ^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$ (field 3 = region); createdAt targets TimestampFormat (@timestampFormat date-time). ResourceNotFoundException requires message, resourceId and resourceType; enum ResourceType = IDENTITY_SOURCE, POLICY_STORE, POLICY, POLICY_TEMPLATE, SCHEMA, POLICY_STORE_ALIAS.

## Steps to Reproduce
1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}); CreatePolicyStoreAlias(aliasName="my-alias", policyStoreId=store) -> aliasArn has an empty region segment (arn:aws:verifiedpermissions::<acct>:policy-store-alias/my-alias) and createdAt is a number (cause 1).
3. GetPolicyStoreAlias(aliasName="my-alias") -> same region-less aliasArn (cause 2).
4. DeletePolicyStoreAlias(aliasName="my-alias"), then GetPolicyStore(policyStoreId="policy-store-alias/my-alias") -> 404 whose resourceType is POLICY_STORE and whose resourceId still carries the "policy-store-alias/" prefix (cause 3).
5. GetPolicyStoreAlias(aliasName="policy-store-alias/my-alias") -> 404 whose resourceId still carries the prefix (cause 4).
6. Re-replay verifiedpermissions-policy-store-alias-workflow steps task#2, task#3, task#10 and task#12.
