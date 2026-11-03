Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-windfarm-trajwire-property-definition-mistakes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-windfarm-trajwire-property-definition-mistakes iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes iotsitewise-windfarm-trajwire-asset-association-mistakes
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|ListAssetModels|4xx->4xx|$.message
Issue-Related: 20261103T025010Z-iotsitewise-asset-hierarchy-and-delete-refusals-answer-the-wrong.md
Issue-Commit: bd9fe9d226ef24de2d25d46b8c18046d76d56a4d

# Validation messages quote the rejected value AWS omits and miscount detected errors

> _Note: this issue was identified against the code base at commit `bd9fe9d226ef24de2d25d46b8c18046d76d56a4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetModel

## Description
The emulator builds its own text for the standard AWS constraint-violation report instead of AWS's. Four separate constraint families diverge in the same two ways: the emulator interpolates the rejected value into the sentence where AWS omits it, and it reports only one violation where AWS reports every violation the member incurred. Every one of these is a 400 with the correct error class, so only the message is wrong — but the message is a requirement.

The shared AWS template for this error class is: '<N> validation error(s) detected: ' followed by one clause per violation, joined by '; '. Each clause reads "Value at '<memberPath>' failed to satisfy constraint: <constraint phrase>". The fixed text is 'validation error detected: ', "Value at '", "' failed to satisfy constraint: " and the constraint phrases ('Member must satisfy regular expression pattern: ', 'Member must have length greater than or equal to ', 'Member must have value greater than or equal to ', 'Member must satisfy enum value set: '); the variable parts are N (the number of violations), the member path, and the limit/pattern/enum-set taken from the model. The rejected VALUE is not part of the template for these members.

Cause 1 of 4: CreateAssetModel with an invalid assetModelProperties[].dataType.
  AWS: Smithy: AssetModelProperty.dataType -> PropertyDataType, required enum. AWS answers 400 with "1 validation error detected: Value at 'assetModelProperties.<n>.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [JSON, BOOLEAN, ANNOTATION, DOUBLE, STRING, STRUCT, VIDEO, INTEGER]" — no rejected value quoted, and the enum set rendered in that order.
  Emulator: emits "1 validation error detected: Value 'double' at 'assetModelProperties.1.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [STRING, INTEGER, DOUBLE, BOOLEAN, STRUCT, VIDEO, ANNOTATION, JSON]" — it inserts "Value '<rejected>' " and renders the set in the model's declaration order (the on-disk PropertyDataType values are STRING, INTEGER, DOUBLE, BOOLEAN, STRUCT, VIDEO, ANNOTATION, JSON).
  Why: clients and conformance suites parse or match this message; a different shape of sentence is a different error to them. Echoing the rejected value also reflects caller input back into an error body AWS keeps value-free.
  Fix: the shared constraint-report serializer used by CreateAssetModel validation — drop the value clause and render the enum value set in AWS's order.
  Traces: iotsitewise-windfarm-trajwire-property-definition-mistakes#task#1 (cluster also covers task#2, task#3, task#4 and iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#4).

Cause 2 of 4: AssociateAssets with a hierarchyId that violates BOTH the @pattern and the @length constraint.
  AWS: Smithy: AssociateAssets.hierarchyId -> CustomID, @length min 13 max 139, @pattern '^(?!00000000-...)[0-9a-f]{8}-...$|^externalId:...'. A value that is both too short and non-matching incurs two violations, and AWS reports both: "2 validation errors detected: Value at 'hierarchyId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>; Value at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13".
  Emulator: reports only the length violation, and quotes the value: "1 validation error detected: Value '<rejected>' at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13".
  Why: a client fixing only the reported violation re-submits and is refused again on the violation it was never told about; the declared count ('2 validation errors detected') is itself part of the contract.
  Fix: the constraint-report serializer must accumulate every violation a member incurs rather than short-circuiting on the first, and order them pattern-then-length as AWS does.
  Traces: iotsitewise-windfarm-trajwire-asset-association-mistakes#task#8 (cluster also covers task#9).

Cause 3 of 4: DescribeAssetModel with an assetModelVersion outside the modelled pattern.
  AWS: Smithy: DescribeAssetModel.assetModelVersion -> AssetModelVersionFilter, @httpQuery assetModelVersion, @pattern '^(LATEST|ACTIVE)$'. AWS answers 400 with "1 validation error detected: Value at 'assetModelVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(LATEST|ACTIVE)$".
  Emulator: same sentence but with the rejected value interpolated: "Value 'active' at 'assetModelVersion' ...". The pattern itself is rendered correctly here.
  Why: as cause 1 — the message differs from AWS's for the same refusal.
  Fix: same serializer; this member confirms the defect is in the shared report builder, not in one operation.
  Traces: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#7.

Cause 4 of 4: ListAssetModels with maxResults below the modelled range.
  AWS: Smithy: ListAssetModels.maxResults -> MaxResults, @httpQuery maxResults, @range min 1 max 250. AWS answers 400 with "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
  Emulator: "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1" — the rejected value again interpolated.
  Why: as cause 1, on the range-constraint phrase, so the fix must cover the range clause too and not only the enum/pattern ones.
  Fix: same serializer (range clause).
  Traces: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#8 (cluster also covers task#9, task#10).

## Evidence
Cause 1: iotsitewise-windfarm-trajwire-property-definition-mistakes#task#1 — expected $.message "1 validation error detected: Value at 'assetModelProperties.1.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [JSON, BOOLEAN, ANNOTATION, DOUBLE, STRING, STRUCT, VIDEO, INTEGER]"; actual "1 validation error detected: Value 'double' at 'assetModelProperties.1.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [STRING, INTEGER, DOUBLE, BOOLEAN, STRUCT, VIDEO, ANNOTATION, JSON]". Both 400.
Cause 2: iotsitewise-windfarm-trajwire-asset-association-mistakes#task#8 — expected "2 validation errors detected: Value at 'hierarchyId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$|^externalId:[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+; Value at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13"; actual "1 validation error detected: Value 'Turbines' at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13". Both 400.
Cause 3: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#7 — expected "...Value at 'assetModelVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(LATEST|ACTIVE)$"; actual "...Value 'active' at 'assetModelVersion' ...". Both 400.
Cause 4: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#8 — expected "...Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"; actual "...Value '0' at 'maxResults' ...". Both 400.
Smithy grounding: PropertyDataType enum values [STRING, INTEGER, DOUBLE, BOOLEAN, STRUCT, VIDEO, ANNOTATION, JSON]; AssetModelProperty.dataType required; AssociateAssets.hierarchyId CustomID @length 13..139 + UUID|externalId @pattern; DescribeAssetModel.assetModelVersion @pattern ^(LATEST|ACTIVE)$; ListAssetModels.maxResults @range 1..250.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Cause 1: CreateAssetModel with assetModelProperties[1].dataType = 'double' (wrong case) — fixture iotsitewise-windfarm-trajwire-property-definition-mistakes task#1; diff $.message.
3. Cause 2: AssociateAssets with hierarchyId = a short non-UUID name — fixture iotsitewise-windfarm-trajwire-asset-association-mistakes task#8; check the leading error count and that both the pattern and the length clause are present.
4. Cause 3: DescribeAssetModel?assetModelVersion=active — fixture iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes task#7.
5. Cause 4: ListAssetModels?maxResults=0 — same fixture, task#8.
6. In each case confirm the message carries no "Value '<rejected>'" clause.
