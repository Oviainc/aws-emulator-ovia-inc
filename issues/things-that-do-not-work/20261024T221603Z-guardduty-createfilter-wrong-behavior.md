Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d2641f8668c2a4abe0c9d800c33fcdc9b555a45a

# Filter and findings criteria refuse 30+ findingCriteria field names the model documents as available for filtering

_Filed: 2026-10-24T22:16:03Z_
_Operation: CreateFilter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d2641f8668c2a4abe0c9d800c33fcdc9b555a45a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every path that accepts a `FindingCriteria` runs it through `crate::state::validate_finding_criteria`, which refuses with HTTP 400 any criterion key that is not one of the 16 entries in `crate::state::FINDING_ATTRIBUTES`. The model documents a far larger set of field names as the ones available for filtering/querying findings — including `resource.instanceDetails.instanceId`, `resource.accessKeyDetails.accessKeyId`, `service.action.actionType`, `service.action.awsApiCallAction.api`, `service.action.dnsRequestAction.domain`, `service.additionalInfo.threatListName`, `service.resourceRole` — and the handler answers the opposite for all of them: a `CreateFilter` naming any of those documented fields cannot be saved at all, and `ListFindings` / `GetFindingsStatistics` / `UpdateFilter` / `GetFindings` (sortCriteria) refuse them the same way.

The refusal is not even limited to attributes the store cannot produce: `service.resourceRole` is persisted on every finding record and reported by `GetFindings`, yet it is absent from the catalogue, so filtering on it is refused too. The result is that the single most common GuardDuty workflow — saving a suppression/archive filter keyed on a `service.action.*` or `resource.*Details.*` field — is impossible against this emulator.

## Evidence

Smithy, `smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`: "Represents the criteria to be used in the filter for querying findings. The following fields are available for filtering:" followed by `accountId`, `arn`, `associatedAttackSequenceArn`, `confidence`, `createdAt`, `id`, `partition`, `region`, `resource.accessKeyDetails.accessKeyId`, ... `resource.containerDetails.id`, `resource.ebsVolumeDetails.scannedVolumeDetails.volumeArn`, `resource.ecsClusterDetails.arn`, ...

Smithy, `smithy_query(query="doc",shape="ListFindingsRequest",member="FindingCriteria")`: "Represents the criteria used for querying findings. Valid values include: ... resource.instanceDetails.instanceId ... service.action.actionType ... service.action.awsApiCallAction.api ... service.additionalInfo.threatListName ... service.resourceRole ... severity ... type ... updatedAt".

The code does the opposite — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:4294-4311:
```
pub const FINDING_ATTRIBUTES: [&str; 16] = [
    "accountId", "arn", "confidence", "createdAt", "description", "id", "partition",
    "region", "resource.resourceType", "schemaVersion", "service.archived",
    "service.detectorId", "severity", "title", "type", "updatedAt",
];
```
state.rs:4501-4504:
```
    for (path, condition) in criterion {
        if !supports_attribute(path) {
            return Err(path.clone());
```
src/ops/create_filter.rs:205-210:
```
        crate::state::validate_finding_criteria(Some(&input.finding_criteria)).map_err(|path| {
            bad_request(format!(
                "findingCriteria.criterion: '{path}' is not a finding attribute this service can filter on. ...
```
The same helper is called from src/ops/update_filter.rs:175, src/ops/list_findings.rs:186, src/ops/get_findings_statistics.rs:209.

An attribute the store DOES hold is refused as well: src/ops/get_findings.rs:221 reports `resource_role: Some(record.resource_role.clone())`, while `service.resourceRole` is not in `FINDING_ATTRIBUTES`.

Test coverage pins the refusal with a reason the model contradicts — tests/all/filter_lifecycle.rs:670-676:
```
    for (path, why) in [
        (
            "resource.instanceDetails.instanceId",
            "a nested path this service does not evaluate",
        ),
```
and tests/all/client_rejections.rs:449-457 does the same for `service.action.actionType` on ListFindings.

## Steps to Reproduce

1. CreateDetector to get a detectorId
2. POST /detector/{detectorId}/filter with body {"name":"suppress-dns","action":"ARCHIVE","findingCriteria":{"criterion":{"service.action.actionType":{"equals":["DNS_REQUEST"]}}}}
3. Observe HTTP 400 BadRequestException "'service.action.actionType' is not a finding attribute this service can filter on", although the CreateFilter model documents the service.action.* fields as available for filtering
4. Repeat with "service.resourceRole" — refused as well, even though the finding record stores resourceRole and GetFindings reports it
5. Same request shape against POST /detector/{detectorId}/findings (ListFindings) is refused identically
