Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# Deduplicate the StoredConnector -> Connector / ConnectorSummary member mapping

_Filed: 2026-10-25T04:05:41Z_
_Operation: GetConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredConnector::to_connector` (what `GetConnector` answers) and `StoredConnector::to_summary` (what `ListConnectors` answers) are two verbatim copies of the same ten-member mapping from the stored record onto the output shape. `Connector` and `ConnectorSummary` model exactly the same ten members (smithy_query shape=Connector,ConnectorSummary), and `to_summary`'s own doc asserts the invariant "It carries the same members as `Connector`, so a listing and a get cannot disagree" — but nothing in the code enforces it: the two copies are independent, so a future change to one (a member newly reported, a value newly derived, a clone newly conditional) silently makes the single read and the listing disagree. The crate already shows the non-duplicated form for its other resource: `StoredChallenge::to_metadata` builds itself FROM `to_metadata_summary` instead of re-listing the members, so the challenge read and the challenge listing provably cannot drift. The connector pair should be derived the same way (one mapping, the other built from it), leaving every response byte and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:317-331:
```
    /// The modelled `Connector`, as `GetConnector` returns it.
    pub fn to_connector(&self) -> Connector {
        Connector {
            arn: Some(self.arn.clone()),
            certificate_authority_arn: Some(self.certificate_authority_arn.clone()),
            r#type: Some(self.connector_type.clone()),
            mobile_device_management: self.mobile_device_management.clone(),
            open_id_configuration: self.open_id_configuration.clone(),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            endpoint: Some(self.endpoint.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
```
Copy 2 — the same ten lines again, state.rs:333-348:
```
    /// The modelled `ConnectorSummary`, as `ListConnectors` returns it. It carries the same
    /// members as `Connector`, so a listing and a get cannot disagree.
    pub fn to_summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            arn: Some(self.arn.clone()),
            certificate_authority_arn: Some(self.certificate_authority_arn.clone()),
            r#type: Some(self.connector_type.clone()),
            ... (identical remaining eight members)
        }
    }
```
The crate's own counter-example for the challenge pair — state.rs:205-216:
```
    /// The modelled `ChallengeMetadata`, as `GetChallengeMetadata` returns it. Its members
    /// are the same four `ChallengeMetadataSummary` carries, ...
    pub fn to_metadata(&self) -> ChallengeMetadata {
        let summary = self.to_metadata_summary();
        ChallengeMetadata {
            arn: summary.arn,
            ...
```
Callers: src/ops/get_connector.rs:167 `connector: Some(stored.to_connector())` and src/ops/list_connectors.rs:187 `(arn.clone(), connector.to_summary())`.

Test coverage that must stay green and unchanged: tests/all/connector_vpc_endpoint.rs:344 ("ListConnectors must report the same Endpoint as GetConnector"), tests/all/connector_lifecycle.rs (connector read-back after create, lines ~424-560, and the paging tables at ~924-1021), tests/all/challenge_metadata.rs:386-413 (the read agrees with the listing member for member).

## Steps to Reproduce

1. Open services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs and compare `to_connector` (lines 317-331) with `to_summary` (lines 333-348): the ten member expressions are identical, duplicated per output shape.
2. Compare with `StoredChallenge::to_metadata` (lines 205-216), which derives itself from `to_metadata_summary` rather than re-listing the members — the same relationship, implemented once.
3. Change one member in `to_connector` only (e.g. stop reporting `status_reason`): GetConnector and ListConnectors now disagree about the same stored record, and only the Endpoint assertion in tests/all/connector_vpc_endpoint.rs:344 cross-checks the two, so the drift can pass the suite.
4. Fix by building one of the two from the other (as the challenge pair already does), keeping both responses byte-identical and all existing tests unchanged.
