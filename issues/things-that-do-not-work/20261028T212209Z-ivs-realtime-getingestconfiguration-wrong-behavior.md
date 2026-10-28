Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# An ingest configuration reports state ACTIVE for a participant that is merely CONNECTED, not publishing — and then blocks its own delete and update

_Filed: 2026-10-28T21:22:09Z_
_Operation: GetIngestConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`IngestConfiguration$state` is computed by `IngestConfigurationRecord::state`, which calls the crate's single "who is broadcasting with this configuration" lookup, `state::ingest_configuration_publishers`. That lookup counts every participant of the configuration's stage that is `CONNECTED` and whose identity the configuration minted (or whose join named its ARN). It never asks whether that participant is PUBLISHING.

The crate already answers that exact question in one place — `StageSessionRecord::is_publishing`, documented as "Whether `participant_id` is publishing to this session right now" and explicitly contrasted with `ParticipantRecord::published` ("whether the participant EVER published"). `join_stage_session` and `record_replicated_participant` both consult it before recording a `PUBLISH_STARTED`/`PUBLISH_STOPPED` occurrence; `ingest_configuration_publishers` — the one reader whose question the model words as "currently is publishing" / "actively streaming" — skips it.

A non-publishing join is a first-class case in this crate: `JoinedParticipant::published` is an input of the one place a participant record is created, and the event suite exercises `Action::Join { published: false }` rows. For such a participant no `PUBLISH_STARTED` event is recorded, so `ListParticipantEvents` reports the participant as having never begun publishing while `GetIngestConfiguration` reports `state: ACTIVE` for the configuration it joined with: two readers of the same store contradict each other.

One helper, three wrong answers:
1. `GetIngestConfiguration` / `ListIngestConfigurations` (and its `filterByState`) report `ACTIVE`.
2. `DeleteIngestConfiguration` without `force` answers `ConflictException` (409) for a configuration nobody is streaming with.
3. `UpdateIngestConfiguration` answers `ConflictException` (409) for the same reason, so the configuration can never be moved to another stage.

## Evidence

The model states the condition as PUBLISHING, not as connected — `smithy_query(query="doc", shape="IngestConfiguration", member="state")`:

> "State of the ingest configuration. It is `ACTIVE` if a publisher currently is publishing to the stage associated with the ingest configuration."

and `smithy_query(query="doc", shape="DeleteIngestConfiguration")`:

> "An IngestConfiguration cannot be deleted if the publisher is actively streaming to a stage, unless `force` is set to `true`."

The lookup checks connection only — `src/state.rs:4145-4157`:

```
    for session in sessions {
        for participant in &session.participants {
            if participant.state != PARTICIPANT_STATE_CONNECTED {
                continue;
            }
            let publishes_with_configuration = configuration.publishes_participant(participant);
            if publishes_with_configuration {
                publishers.push(IngestPublisher {
```

and the predicate it calls is about IDENTITY, not about media — `src/state.rs:1161-1164`:

```
    pub fn publishes_participant(&self, record: &ParticipantRecord) -> bool {
        record.ingest_configuration_arn.as_deref() == Some(self.arn.as_str())
            || self.mints_participant(&record.participant_id)
    }
```

The crate's own single answer to "is this participant publishing right now", which this path does not call — `src/state.rs:2591-2609`:

```
    /// Whether `participant_id` is publishing to this session right now.
    ///
    /// Read off the occurrences themselves: [`EVENT_PUBLISH_STARTED`] and
    /// [`EVENT_PUBLISH_STOPPED`] are what begin and end one participant's media, so the
    /// later of the two is its publishing state and the two alternate rather than
    /// repeating. [`ParticipantRecord::published`] cannot answer this — it is "whether
    /// the participant EVER published", which stays set once set.
    pub fn is_publishing(&self, participant_id: &str) -> bool {
```

The `state` member is read off that lookup — `src/state.rs:1116-1125`:

```
    pub fn state(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> String {
        if ingest_configuration_publishers(store, self).is_empty() {
            NO_PUBLISHER_INGEST_CONFIGURATION_STATE.to_string()
        } else {
            PUBLISHING_INGEST_CONFIGURATION_STATE.to_string()
```

and the two refusals hang off the same lookup — `src/ops/delete_ingest_configuration.rs:181-192` ("the publishers are the ones the configuration's stage holds right now") and `src/ops/update_ingest_configuration.rs:210-221`:

```
        if let Some(publisher) =
            crate::state::ingest_configuration_publishers(&store, &record).first()
        {
            return Err(crate::state::conflict(
```

Test coverage: every ingest-configuration suite seeds the publisher with `published: true` (`tests/all/ingest_configurations.rs`, `tests/all/delete_ingest_configuration.rs:549`, `tests/all/update_ingest_configuration.rs:772`, `tests/all/participant_ingest_configuration.rs:342`), so no test pins the connected-but-not-publishing case; `tests/all/participant_events.rs:329` shows a non-publishing join is a modelled case of this crate (`Action::Join { published: false }` records only `JOINED`).

## Steps to Reproduce

1. `CreateIngestConfiguration{ingestProtocol: "RTMPS", stageArn: <a stage S created by CreateStage>, insecureIngest: false, redundantIngest: false}` → note the returned `ingestConfiguration.arn` (A) and `ingestConfiguration.participantId` (pA); the answer reports `state: "INACTIVE"`.
2. Through the data-plane seam every test suite of this crate uses, record pA joining S WITHOUT publishing: `state::join_stage_session(&mut store, S, state::JoinedParticipant { participant_id: pA, published: false, ..Default::default() })`.
3. `ListParticipantEvents{stageArn: S, sessionId: <S activeSessionId>, participantId: pA}` → only a `JOINED` event; no `PUBLISH_STARTED`, i.e. the store records nothing being published.
4. `GetIngestConfiguration{arn: A}` → `state: "ACTIVE"`, although the model says ACTIVE means "a publisher currently is publishing" and the store's own event log says pA never began publishing. `ListIngestConfigurations{filterByState: "ACTIVE"}` reports it too.
5. `DeleteIngestConfiguration{arn: A}` (no `force`) → HTTP 409 ConflictException, although the model refuses only when "the publisher is actively streaming to a stage".
6. `UpdateIngestConfiguration{arn: A, stageArn: ""}` → HTTP 409 ConflictException, so the configuration can never be re-associated.
