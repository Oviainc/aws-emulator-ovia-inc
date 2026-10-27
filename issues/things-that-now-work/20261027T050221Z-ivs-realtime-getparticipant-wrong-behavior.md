Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# Participant reports no ingestConfigurationArn for a publisher that reached the stage on an ingest credential, while protocol and IngestConfiguration.state resolve it

_Filed: 2026-10-27T05:02:21Z_
_Operation: GetParticipant | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Participant`/`ParticipantSummary` report `ingestConfigurationArn` straight off the stored `ParticipantRecord::ingest_configuration_arn`, i.e. only the ARN the join itself named. Every other reading of "which ingest configuration does this participant broadcast with" goes through `state::participant_ingest_configuration`, which also resolves a configuration whose ingest credentials minted that participant id (`IngestConfigurationRecord::publishes_participant`). `Participant$protocol` uses that helper; `ingestConfigurationArn` does not.

So for a publisher that reached the stage on the configuration's primary or redundant ingest credential — an identity the crate explicitly supports, and which `ingest_configuration_publishers` counts, making `GetIngestConfiguration` report `state: ACTIVE` and `DeleteIngestConfiguration` refuse with ConflictException — GetParticipant answers `protocol: "RTMPS"` (the configuration's) and no `ingestConfigurationArn` at all. The participant read therefore contradicts the configuration read about the same pair, which is precisely what the helper's own doc says cannot happen.

## Evidence

The helper and its promise, src/state.rs:1920-1928:
```
/// The ingest configuration the participant `record` of `stage_arn` broadcasts with, or
/// `None` when it broadcasts with none.
///
/// The one its join named, and otherwise one of `stage_arn`'s configurations whose ingest
/// credentials were minted for that participant — the one predicate
/// [`IngestConfigurationRecord::publishes_participant`] states, which
/// [`ingest_configuration_publishers`] answers in the other direction, so what a read of
/// a participant says about its ingest and what a read of the configuration says about
/// its publishers cannot disagree.
```
One path uses it, the other does not — src/state.rs:2045 and src/state.rs:2051 inside `ParticipantRecord::to_participant`:
```
            protocol: Some(participant_protocol(store, stage_arn, self)),
...
            ingest_configuration_arn: self.ingest_configuration_arn.clone(),
```
(the same bypass in `to_summary`, src/state.rs:1996: `            ingest_configuration_arn: self.ingest_configuration_arn.clone(),`).

The predicate the other direction uses, src/state.rs:1150-1152:
```
    pub fn publishes_participant(&self, record: &ParticipantRecord) -> bool {
        record.ingest_configuration_arn.as_deref() == Some(self.arn.as_str())
            || self.mints_participant(&record.participant_id)
```
and src/state.rs:1109-1113, which turns that into the reported `IngestConfiguration$state` (`ACTIVE`).

That the credential-only identity is a supported, exercised case: tests/all/delete_ingest_configuration.rs:524-539 enumerates `PublisherIdentity::PrimaryCredential` / `RedundantCredential` with `(member(&configuration, "participantId"), None)` and seeds the join with `ingest_configuration_arn: configuration_of_join` (= `None`) at tests/all/delete_ingest_configuration.rs:551.

Smithy: `smithy_query(query="doc", shape="Participant", member="ingestConfigurationArn")` — "The participant’s ingest configuration."

Test coverage: tests/all/get_participant.rs:886-891 asserts `ingestConfigurationArn` only for a join that NAMED the ARN (`ingest_configuration_arn: Some(configuration_arn.clone())`, line 864), so the credential-only case is unpinned.

## Steps to Reproduce

1. CreateStage, then CreateIngestConfiguration with that stageArn and redundantIngest=true; note `ingestConfiguration.participantId` and `arn`.
2. Record the publisher joining as that participantId with `ingest_configuration_arn: None` (the data-plane seam `state::join_stage_session`, exactly as tests/all/delete_ingest_configuration.rs:544-554 does for PublisherIdentity::PrimaryCredential).
3. GetIngestConfiguration on the configuration: it answers `state: "ACTIVE"`, i.e. this participant publishes with it.
4. GetStage to read activeSessionId, then GetParticipant for that participantId.
5. Expected: `ingestConfigurationArn` naming that configuration — the one the crate's own `participant_ingest_configuration` resolves and the one `protocol` is read from.
6. Actual: `protocol` is the configuration's ingest protocol but `ingestConfigurationArn` is absent (and absent in the ListParticipants summary too), so the participant read and the configuration read disagree.
