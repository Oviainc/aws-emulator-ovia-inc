Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# PutResourcePermission grants RESTORE on ARNs that name nothing: the modeled ResourceNotFoundException is unreachable

_Filed: 2026-10-24T15:17:25Z_
_Operation: PutResourcePermission | Class: validation_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: PutResourcePermission attaches a RESTORE permission to an existing SSM-for-SAP resource (a registered HANA database) naming an existing source resource; an ARN that names no such resource is refused with the modeled ResourceNotFoundException (HTTP 404, "The resource is not available").

Actual: the handler performs no resolution at all. Beyond the model's own @required/@pattern checks it stores the (ResourceArn, ActionType, SourceResourceArn) triple for any syntactically valid ARN — including ARNs of other services (arn:aws:s3:::bucket/key as the target) and database ARNs of applications that were never registered — and answers 200 with a rendered policy document. GetResourcePermission then reports that policy, so the emulator advertises a permission on a resource that does not exist. ResourceNotFoundException is declared for both PutResourcePermission and GetResourcePermission and can never be produced. Note that the handler's stated justification is stale: it says database resolution is impossible because "both GetDatabase and ListDatabases are still scaffolds", but ListDatabases is implemented and database ARNs are computed by crate::state::database_arn, so the target ARN of a database of a registered application is now derivable from state.

## Evidence

Smithy model: PutResourcePermission errors = ConflictException-free set {InternalServerException 500, ResourceNotFoundException 404, ValidationException 400}; ResourceArn/SourceResourceArn are SsmSapArn. Handler: src/ops/put_resource_permission.rs — the only checks are the MAX_REGIONS ceiling and the store ceilings inside RegionState::grant_resource_permission (both reported as ValidationException); no lookup against RegionState::applications or StoredApplication::databases. src/state.rs resource_policy/grant_resource_permission are keyed by raw ARN strings. Tests: tests/all/resource_permissions.rs deliberately grants on ARNs of applications that were never registered (e.g. arn:aws:ssm-sap:us-east-1:...:database/HDB-TEST/HDB and arn:aws:s3:::sap-backint-bucket/backups), pinning the permissive behavior; no test asserts a 404 for an unknown resource.

## Steps to Reproduce

1. Start with an empty store (no application registered)
2. POST /put-resource-permission with {"ActionType":"RESTORE","ResourceArn":"arn:aws:ssm-sap:us-east-1:000000000000:database/NOPE/NOPE","SourceResourceArn":"arn:aws:ssm-sap:us-east-1:000000000000:database/ALSO-NOPE/NOPE"}
3. Observe 200 with a Policy document, where the real service answers 404 ResourceNotFoundException because neither ARN names an SSM-for-SAP resource
4. POST /get-resource-permission for the same ResourceArn and observe the fabricated policy being reported back
