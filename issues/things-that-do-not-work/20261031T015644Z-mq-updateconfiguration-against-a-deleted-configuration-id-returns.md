Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mq.delete-configuration-removes-it
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq.delete-configuration-removes-it|run#16|AssertionError|assert update_code == "ConflictException", (
Issue-Commit: 36f39162ebee3bd1593b36ec8bda057b2a67c056

# mq UpdateConfiguration against a DELETED configuration id returns NotFoundException instead of ConflictException

> _Note: this issue was identified against the code base at commit `36f39162ebee3bd1593b36ec8bda057b2a67c056`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfiguration

## Description
One root cause, grounded in one failing behavioral check.

Cause 1 of 1: a mutation of a configuration whose id was issued and then deleted is refused as not-found, where Amazon MQ refuses it as a conflict with the configuration's deleted state.
Failing check: mq.delete-configuration-removes-it

AWS: Smithy (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json): UpdateConfiguration PUT /v1/configurations/{ConfigurationId}, required `ConfigurationId` (httpLabel) and `Data`, success 200; modelled errors BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, InternalServerErrorException 500. DeleteConfiguration DELETE /v1/configurations/{ConfigurationId} models the same error set. The model does not say which of NotFoundException/ConflictException covers a deleted id, so this is recorded behavior only: the recording shows that after DeleteConfiguration succeeds, the two surfaces diverge for the SAME id — a READ (DescribeConfiguration) is refused HTTP 404 `NotFoundException` with message `Configuration ID [<configuration-id>] can't be found. Make sure it exists.`, while a MUTATION (UpdateConfiguration) is refused HTTP 409 `ConflictException` with message `Concurrent update to Configuration [<configuration-id>]. Retry to create a new revision.` Amazon MQ does not forget a once-issued configuration id on delete; it holds it in a deleted state and treats a write against that state as a revision conflict, not as an unknown resource. Message decomposition for the ConflictException: the fixed text is `Concurrent update to Configuration [` + `]. Retry to create a new revision.`; the ONLY variable part is the configuration id, taken from the request's `ConfigurationId` (the value in any single replay, e.g. a `c-...` id, fills that slot and is not part of the requirement).

Emulator: UpdateConfiguration answers HTTP 404 `NotFoundException` (errorAttribute `configurationId`, message `Configuration ID [<configuration-id>] can't be found. Make sure it exists.`) — the same refusal the read path gives. The check's earlier assertions pass (DescribeConfiguration on the deleted id is correctly 404 NotFoundException with the documented sentence, and ListConfigurations no longer lists the name), so the divergence is isolated to the mutation path's classification. Root cause: DeleteConfiguration hard-removes the record (`store.configurations.remove(&input.configuration_id)`), so no trace of the id survives; UpdateConfiguration's lookup miss then falls into its generic not-found branch and cannot tell "never existed" from "deleted".

Note: an OPEN issue, 20261030T170907Z-mq-configuration-operations-createconfiguration-wrongly-refuses-a-duplicate.md, carries UpdateConfiguration as a scaffold stub (501) plus revision-concurrency semantics; the emulator has since drifted — UpdateConfiguration is implemented and serves revisions — and that issue's replay keys (mq.create-configuration-duplicate-name-conflict-rejected, mq.update-configuration-revision-concurrency) do not include this check and it says nothing about the deleted-id classification. This is a separate cause.

Why: a client that deletes a configuration and then retries a queued or in-flight revision push receives 404 instead of 409. 404 reads as "this id was never mine / I targeted the wrong account or region", so idempotent IaC and rollout controllers take the wrong branch: they re-create or re-resolve the configuration rather than treating the write as a lost race against the configuration's state and surfacing a conflict. Retry logic keyed on ConflictException never fires, and conflict-vs-missing telemetry mis-attributes the failure.

Fix: the deleted-configuration state belongs in this service's state (services/service-com-amazonaws-mq-mq/src/state.rs) — DeleteConfiguration (services/service-com-amazonaws-mq-mq/src/ops/delete_configuration.rs, the `store.configurations.remove(...)` branch) must retire the record rather than drop the id, and UpdateConfiguration's handler (services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs, the `store.configurations.get_mut(&input.configuration_id)` miss branch) must distinguish a never-issued id (keep NotFoundException 404) from a retired one (ConflictException 409 with the sentence above). Read paths (DescribeConfiguration, ListConfigurations, ListConfigurationRevisions) must keep their current behavior: 404 / absent.

## Evidence
Expected (AWS, per the recorded check):
  after mq.delete_configuration(ConfigurationId=<id>) succeeds,
  mq.update_configuration(ConfigurationId=<id>, Data=<base64 ActiveMQ broker XML>)
  -> HTTP 409, Error.Code == "ConflictException",
     Error.Message == "Concurrent update to Configuration [<id>]. Retry to create a new revision."

Actual (emulator):
  -> HTTP 404, Error.Code == "NotFoundException",
     Error.Message == "Configuration ID [<id>] can't be found. Make sure it exists."

Replay assertion (check_mq_delete_configuration_removes_it.py line 51):
  AssertionError: expected UpdateConfiguration against a deleted id to return ConflictException, got NotFoundException

Passing in the same check (not part of the defect): DescribeConfiguration on the deleted id -> NotFoundException with "Configuration ID [<id>] can't be found. Make sure it exists."; the deleted name absent from ListConfigurations.

Source of the divergence:
  services/service-com-amazonaws-mq-mq/src/ops/delete_configuration.rs:161 — hard `remove`, no retired/tombstone state.
  services/service-com-amazonaws-mq-mq/src/ops/update_configuration.rs:187-198 — lookup miss returns NotFoundException unconditionally.

## Steps to Reproduce
1. mq.create_configuration(EngineType="ACTIVEMQ", Name="<any-valid-name>") -> note Id.
2. mq.describe_configuration(ConfigurationId=<Id>) -> 200, echoes Id (sanity).
3. mq.delete_configuration(ConfigurationId=<Id>) -> 200.
4. mq.describe_configuration(ConfigurationId=<Id>) -> expected and observed: 404 NotFoundException, "Configuration ID [<Id>] can't be found. Make sure it exists."
5. mq.update_configuration(ConfigurationId=<Id>, Data=base64('<broker xmlns="http://activemq.apache.org/schema/core"><plugins/></broker>'))
   Expected: HTTP 409 ConflictException, "Concurrent update to Configuration [<Id>]. Retry to create a new revision."
   Observed: HTTP 404 NotFoundException, "Configuration ID [<Id>] can't be found. Make sure it exists."
Replay: mq.delete-configuration-removes-it
