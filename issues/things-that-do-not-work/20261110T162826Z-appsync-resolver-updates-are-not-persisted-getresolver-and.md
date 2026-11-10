Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-datasource-function-resolver-pipeline
Issue-Step: task#17
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-datasource-function-resolver-pipeline appsync-cfg-resolver-sync-config-rules appsync-unit-resolver-code-evaluation
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|GetResolver|2xx->2xx|$.resolver.requestMappingTemplate
Issue-Key: appsync|ListResolvers|2xx->2xx|$.resolvers[*]
Issue-Key: appsync|ListResolversByFunction|2xx->2xx|$.resolvers[*]
Issue-Key: appsync|GetResolver|2xx->2xx|$.resolver.syncConfig.conflictDetection
Issue-Key: appsync|GetResolver|2xx->2xx|$.resolver.syncConfig.conflictHandler
Issue-Key: appsync|GetResolver|2xx->2xx|$.resolver.code
Issue-Commit: e88affa0c81a58fb8423ccb328f6804dccd04335

# AppSync resolver updates are not persisted: GetResolver and the resolver lists serve pre-update state (5 causes)

> _Note: this issue was identified against the code base at commit `e88affa0c81a58fb8423ccb328f6804dccd04335`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateResolver

## Description
Five net-new causes, all one root fault: an accepted UpdateResolver mutation is not written into resolver state, so every later read of that resolver (GetResolver, ListResolvers, ListResolversByFunction) serves the values the resolver had BEFORE the update. In each fixture the UpdateResolver step itself replayed clean — only the subsequent reads diverge, so the update is answered but not stored (or the readers serve a stale copy).

Cause 1 of 5 — GetResolver serves the pre-update requestMappingTemplate.
AWS: Smithy: UpdateResolver (POST /v1/apis/{apiId}/types/{typeName}/resolvers/{fieldName}) takes requestMappingTemplate (MappingTemplate, @length 1..65536) and returns the updated Resolver; GetResolver returns that same Resolver. AWS reads back exactly the template the accepted update carried, byte for byte.
Emulator: after an UpdateResolver that set a new requestMappingTemplate, GetResolver answers 200 with the template the resolver was CREATED with ($.resolver.requestMappingTemplate: expected the updated template, got the original one — in the recording expected '{\"updated\": true}', got '{}').
Why: a client that updates a resolver and reads it back (the normal confirm-after-write pattern, and what CloudFormation/CDK drift checks do) sees its change silently lost, and any tooling that re-applies on mismatch loops forever.
Fix: UpdateResolver's handler/state write — the resolver record in the AppSync control-plane state must be replaced with the merged updated members, not left at creation values.
Traces: appsync-datasource-function-resolver-pipeline#task#17.

Cause 2 of 5 — ListResolvers returns the pre-update resolver element.
AWS: Smithy: ListResolvers (GET /v1/apis/{apiId}/types/{typeName}/resolvers) returns Resolvers, the same Resolver structures Get/UpdateResolver return, so the listed element carries the updated requestMappingTemplate.
Emulator: the listed element for the updated resolver still carries the creation-time requestMappingTemplate (expected the updated template, got the original). The element additionally omits the Resolver members the recording carries as unset (dataSourceName, syncConfig, cachingConfig, runtime, code, metricsConfig); the semantic divergence is the stale template — the omitted-null shape overlaps open issue 20261029T200818Z-appsync-response-projections-omit-handlerconfigs-and-emit-members.md, which does not own this step.
Why: list-based reconciliation (console resolver list, IaC drift detection) reports the old template for a resolver that was successfully updated.
Fix: same resolver state write as Cause 1; ListResolvers' serializer must project the stored record.
Traces: appsync-datasource-function-resolver-pipeline#task#18.

Cause 3 of 5 — ListResolversByFunction returns the pre-update resolver element.
AWS: Smithy: ListResolversByFunction (GET /v1/apis/{apiId}/functions/{functionId}/resolvers) returns the same Resolvers list, hence the updated template.
Emulator: identical stale element to Cause 2 (expected updated requestMappingTemplate, got the creation-time one) with the same unset members omitted.
Why: a client enumerating the resolvers that reference a pipeline function sees pre-update definitions and cannot tell an applied update from a dropped one.
Fix: same resolver state write as Cause 1; ListResolversByFunction's serializer must project the stored record.
Traces: appsync-datasource-function-resolver-pipeline#task#19.

Cause 4 of 5 — GetResolver serves a syncConfig the resolver no longer has.
AWS: Smithy: SyncConfig.conflictHandler (ConflictHandlerType) / conflictDetection (ConflictDetectionType) are set by Create/UpdateResolver and read back by GetResolver; a resolver whose last accepted call set conflictHandler OPTIMISTIC_CONCURRENCY and conflictDetection NONE reads back with exactly those values.
Emulator: GetResolver answers 200 with $.resolver.syncConfig.conflictHandler 'LAMBDA' and conflictDetection 'VERSION' where AWS reports 'OPTIMISTIC_CONCURRENCY' and 'NONE' — i.e. the emulator keeps syncConfig values from an earlier call instead of the state AWS ends in. Hypothesis: the same non-persisted-update path, with the extra possibility that a syncConfig AWS refused (LAMBDA without lambdaConflictHandlerConfig) was retained; the related wrong-message refusal in this fixture is owned by 20261029T200818Z-appsync-validation-refusals-are-emulator-authored-wrong-message.md, which does not own this read step.
Why: Conflict Detection/Resolution is the behaviour of a sync-enabled resolver; a client reading back LAMBDA/VERSION where AWS has OPTIMISTIC_CONCURRENCY/NONE configures or audits the wrong conflict strategy.
Fix: UpdateResolver's state write plus its syncConfig validation — the stored syncConfig must be the one the last ACCEPTED call set, and nothing else.
Traces: appsync-cfg-resolver-sync-config-rules#task#12.

Cause 5 of 5 — GetResolver serves the pre-update APPSYNC_JS code.
AWS: Smithy: Resolver.code (Code, @length 1..32768) with runtime AppSyncRuntime; an accepted Create/UpdateResolver carrying code makes GetResolver read that code back unchanged.
Emulator: GetResolver answers 200 with the earlier code body (expected the handler that returns the args payload, got the earlier handler returning an empty object) — the newer accepted code was never stored.
Why: a client that uploads resolver code and reads it back to verify the deployment sees the previous revision, so a code rollout silently does not take effect.
Fix: UpdateResolver's state write for code/runtime members.
Traces: appsync-unit-resolver-code-evaluation#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):
- appsync-datasource-function-resolver-pipeline#task#17 GetResolver 200->200: $.resolver.requestMappingTemplate value_mismatch (expected '{"updated": true}', got '{}').
- appsync-datasource-function-resolver-pipeline#task#18 ListResolvers 200->200: $.resolvers[0] differs — requestMappingTemplate '{"updated": true}' expected, '{}' served; keys dataSourceName/syncConfig/cachingConfig/runtime/code/metricsConfig present-as-null in the recording, absent in the emulator response.
- appsync-datasource-function-resolver-pipeline#task#19 ListResolversByFunction 200->200: same element divergence.
- appsync-cfg-resolver-sync-config-rules#task#12 GetResolver 200->200: $.resolver.syncConfig.conflictHandler expected 'OPTIMISTIC_CONCURRENCY', got 'LAMBDA'; $.resolver.syncConfig.conflictDetection expected 'NONE', got 'VERSION'.
- appsync-unit-resolver-code-evaluation#task#12 GetResolver 200->200: $.resolver.code expected 'export function request(ctx) {\n  return { payload: ctx.args };\n}\nexport function response(ctx) {\n  return ctx.result;\n}\n', got 'export function request(ctx) {\n  return {};\n}\nexport function response(ctx) {\n  return ctx.result;\n}\n'.
In all five the UpdateResolver step that made the change replayed without divergence, so the mutation was accepted on the wire and lost in state.

## Steps to Reproduce
1. CreateGraphqlApi, create a data source/function as the fixture does, then CreateResolver on type Query field getItem with requestMappingTemplate '{}'.
2. Call UpdateResolver on the same apiId/typeName/fieldName with requestMappingTemplate '{"updated": true}' — it answers 200.
3. Call GetResolver for that resolver: the emulator returns the '{}' template instead of '{"updated": true}' (cause 1). ListResolvers for the type and ListResolversByFunction for the pipeline function return the same stale element (causes 2-3).
4. Repeat with an UpdateResolver that sets syncConfig {conflictHandler: OPTIMISTIC_CONCURRENCY, conflictDetection: NONE} on a resolver previously configured LAMBDA/VERSION, then GetResolver: LAMBDA/VERSION are still served (cause 4).
5. Repeat with an UpdateResolver that sets a new APPSYNC_JS code body, then GetResolver: the previous code body is served (cause 5).
Replay: appsync-datasource-function-resolver-pipeline (task#17, task#18, task#19), appsync-cfg-resolver-sync-config-rules (task#12), appsync-unit-resolver-code-evaluation (task#12).
