Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-get-network-migration-definition-nonexistent-rejected
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-get-network-migration-definition-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|GetNetworkMigrationDefinition|4xx->4xx|$.message
Issue-Key: mgn|GetNetworkMigrationDefinition|4xx->4xx|$.resourceType
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# mgn ResourceNotFoundException reports a kebab-case resourceType instead of the AWS resource type name

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkMigrationDefinition

## Description
Cause 1 of 1: mgn's ResourceNotFoundException body carries an emulator-internal, kebab-cased identifier in its modeled `resourceType` member rather than the AWS resource type name, and the accompanying `message` is not in AWS's shape either.

AWS: Smithy (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model) models GetNetworkMigrationDefinition with error ResourceNotFoundException @httpError 404, whose members are `message`, `code`, `resourceId` ("Resource ID not found error.") and `resourceType` ("Resource type not found error."), all LargeBoundedString. The operation output shape is NetworkMigrationDefinition and references resource com.amazonaws.mgn#NetworkMigrationDefinitionResource. The model does not enumerate resourceType values, so for the exact value this is recorded behavior only: the recording shows AWS answering an unknown `nmd-` id with HTTP 404, resourceType = "NetworkMigrationDefinition" — the PascalCase AWS resource type name, matching the modeled resource — and a message of the form "<ResourceType> with Id: '<requestedId>' was not found" (example: "NetworkMigrationDefinition with Id: 'nmd-00000000000000000' was not found"). That message is built from the request's id plus the same resource type name, so the rule, not the literal text, is the requirement.

Emulator: returns the correct HTTP 404 and ResourceNotFoundException, but populates resourceType with the emulator's internal kebab-case route/resource slug "network-migration-definition" and emits an unrelated message format ("Network migration definition nmd-00000000000000000 does not exist."), which neither names the resource type as AWS spells it nor follows the "<ResourceType> with Id: '<id>' was not found" construction.

Why: resourceType is a structured, machine-readable member of the modeled error — clients and migration tooling branch on it to decide which resource kind was missing, aggregate not-found errors per type, and surface the resource name in UI. A kebab-case internal slug does not equal any AWS resource type name, so such comparisons silently never match and the error is mishandled as an unknown resource kind. Operators parsing the message for the missing id likewise get nothing.

Fix: the GetNetworkMigrationDefinition handler's not-found error construction in the mgn service crate — where the ResourceNotFoundException is built, the resourceType should be populated with the AWS resource type name ("NetworkMigrationDefinition") rather than the internal kebab-case slug, and the message composed from that type name plus the requested id. If other mgn lookups share one error-construction helper keyed by an internal slug, that mapping is the right place to fix so every mgn resource reports its AWS type name.

Traces: mgn-get-network-migration-definition-nonexistent-rejected#task#1 (expected 404 with resourceType "NetworkMigrationDefinition"; got 404 with resourceType "network-migration-definition").

Related but not covering this: open issue 20261023T003710Z-mgn-answers-pattern-violating-request-bodies-with-validationexception.md concerns the wrong error SHAPE for pattern-violating input; this divergence is inside a correctly-chosen ResourceNotFoundException's payload.

## Evidence
Fixture mgn-get-network-migration-definition-nonexistent-rejected, step task#1, operation GetNetworkMigrationDefinition.
Status: expected 404, got 404 (status and error shape correct).
Body field divergences reported by replay:
  $.resourceType: value_mismatch — expected 'NetworkMigrationDefinition', got 'network-migration-definition'
  $.message: value_mismatch — expected "NetworkMigrationDefinition with Id: 'nmd-00000000000000000' was not found", got 'Network migration definition nmd-00000000000000000 does not exist.' (recorded message quoted as an example; the requirement is the "<ResourceType> with Id: '<id>' was not found" construction over the requested id)
Model reference: ResourceNotFoundException { message, code, resourceId, resourceType } @httpError 404, bound as an error of GetNetworkMigrationDefinition (POST /network-migration/GetNetworkMigrationDefinition); input networkMigrationDefinitionID pattern ^nmd-[0-9a-zA-Z]{17}$, so 'nmd-00000000000000000' is a well-formed but nonexistent id.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture mgn-get-network-migration-definition-nonexistent-rejected, step task#1: POST /network-migration/GetNetworkMigrationDefinition against the mgn endpoint with body {"networkMigrationDefinitionID": "nmd-00000000000000000"} (a well-formed id that was never created).
3. Observe HTTP 404 with a ResourceNotFoundException body; inspect the JSON payload.
4. resourceType comes back as 'network-migration-definition' instead of AWS's 'NetworkMigrationDefinition', and the message is 'Network migration definition nmd-00000000000000000 does not exist.' instead of being composed as "NetworkMigrationDefinition with Id: 'nmd-00000000000000000' was not found".
