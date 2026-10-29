Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-stage-access-log-settings-asymmetric-patch
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-stage-access-log-settings-asymmetric-patch apigateway-stage-cache-cluster-flags-and-patch-op-grammar apigateway-stage-canary-settings-promote-and-remove
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetStage|2xx->2xx|$.accessLogSettings.destinationArn
Issue-Key: apigateway|GetStage|2xx->2xx|$.accessLogSettings.format
Issue-Key: apigateway|GetStage|2xx->2xx|$.cacheClusterSize
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.cacheClusterSize
Issue-Key: apigateway|GetStage|2xx->2xx|$.canarySettings
Issue-Key: apigateway|GetStage|2xx->2xx|$.deploymentId
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.canarySettings.deploymentId
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.deploymentId
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.canarySettings
Issue-Key: apigateway|UpdateStage|2xx->4xx|$.variables
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.variables.color
Issue-Key: apigateway|GetStage|2xx->2xx|$.variables.color
Issue-Related: 20261029T200808Z-apigateway-api-gateway-patchoperations-grammar-op-enum-precedence-and.md
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway stage state: accessLogSettings, cacheCluster and canary patches (9 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStage

## Description
A Stage's patchable state is stored and serialised wrongly: access-log settings, the cache-cluster size and the whole canary-release lifecycle. Smithy: Stage carries accessLogSettings (AccessLogSettings), cacheClusterSize (CacheClusterSize), canarySettings (CanarySettings {percentTraffic, deploymentId, stageVariableOverrides, useStageCache}) and variables (MapOfStringToString); UpdateStage/GetStage return the full Stage.

Cause 1 of 9 - accessLogSettings is applied asymmetrically. AWS: after the recorded add/replace/remove sequence the stage reports accessLogSettings.format (the format string from the request) and NO destinationArn. Emulator: format is absent and the previously-removed destinationArn is still present. Why: a client cannot tell which access-log fields are configured. Fix: UpdateStage's accessLogSettings add/remove handling plus Stage serialisation. Trace: apigateway-stage-access-log-settings-asymmetric-patch#task#12.

Cause 2 of 9 - cacheClusterSize is not persisted. AWS: GetStage echoes the value set by the preceding patch. Emulator: the member is absent. Fix: stage state write for /cacheClusterSize. Trace: apigateway-stage-cache-cluster-flags-and-patch-op-grammar#task#10.

Cause 3 of 9 - UpdateStage's own response omits cacheClusterSize for the same reason. Trace: apigateway-stage-cache-cluster-flags-and-patch-op-grammar#task#14.

Cause 4 of 9 - canarySettings never materialise, and the base deploymentId is overwritten. AWS: after creating a canary the Stage carries canarySettings {deploymentId=<canary deployment>, percentTraffic, useStageCache} while the top-level deploymentId stays the stage's base deployment. Emulator: canarySettings absent and the top-level deploymentId replaced by the canary's deployment id. Why: a canary release is indistinguishable from a full promotion - clients see 100% of traffic on the canary deployment. Fix: Stage canary state + serialiser. Trace: apigateway-stage-canary-settings-promote-and-remove#task#8.

Cause 5 of 9 - UpdateStage response shows the same defect (canarySettings.deploymentId missing, base deploymentId overwritten). Trace: apigateway-stage-canary-settings-promote-and-remove#task#9.

Cause 6 of 9 - a canary patch (percentTraffic / stageVariableOverrides / useStageCache) is refused. AWS: 200 with canarySettings {percentTraffic, stageVariableOverrides, useStageCache} and the stage's variables intact. Emulator: 400. Why: the canary cannot be configured at all. Fix: UpdateStage canary patch paths. Trace: apigateway-stage-canary-settings-promote-and-remove#task#13.

Cause 7 of 9 - promoting a canary does not apply its stageVariableOverrides. AWS: after the promote patch the stage's variables carry the overridden value. Emulator: the pre-promotion value survives. Fix: canary promotion in the stage state. Trace: apigateway-stage-canary-settings-promote-and-remove#task#15.

Cause 8 of 9 - GetStage shows the same unpromoted variables. Trace: apigateway-stage-canary-settings-promote-and-remove#task#16.

Cause 9 of 9 - removing a canary leaves the stage with neither the promoted variables nor canarySettings.deploymentId. Trace: apigateway-stage-canary-settings-promote-and-remove#task#18.

Values quoted in the evidence (deployment ids, variable values, stage names) are that recording's instances of the rule, not requirements. No open issue owns these clusters.

## Evidence
Cause 1: $.accessLogSettings.format missing (expected the request's format string), $.accessLogSettings.destinationArn present though expected absent.
Causes 2/3: $.cacheClusterSize missing where the recording echoes the patched value.
Cause 4: $.canarySettings missing (expected {deploymentId, percentTraffic, useStageCache}); $.deploymentId reports the canary deployment instead of the base deployment.
Cause 5: $.canarySettings.deploymentId missing; $.deploymentId likewise swapped.
Cause 6: status expected 200 with canarySettings {percentTraffic, stageVariableOverrides, useStageCache} and variables intact, got 400.
Causes 7-9: $.variables.<key> still reports the pre-promotion value; $.canarySettings.deploymentId missing.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Cause 1: run fixture apigateway-stage-access-log-settings-asymmetric-patch and read GetStage at step task#12.
3. Causes 4-9: run apigateway-stage-canary-settings-promote-and-remove end to end - create a canary, patch percentTraffic/stageVariableOverrides, promote, then remove - comparing canarySettings, deploymentId and variables at each step.
