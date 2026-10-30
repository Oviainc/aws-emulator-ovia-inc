Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-configuration-bundle-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-configuration-bundle-error-paths bedrock-agentcore-control-configuration-bundle-errors bedrock-agentcore-control-configuration-bundle-lifecycle bedrock-agentcore-control-configuration-bundle-round-trip
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetConfigurationBundle|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetConfigurationBundle|4xx->2xx|$
Issue-Key: bedrock-agentcore-control|GetConfigurationBundle|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetConfigurationBundle|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetConfigurationBundle|4xx->2xx|status
Issue-Commit: f7507ccd8e733df5ba5b0e6ba5890532707d636f

# Configuration bundle not-found path diverges: invented 404 message and deleted bundles still readable

> _Note: this issue was identified against the code base at commit `f7507ccd8e733df5ba5b0e6ba5890532707d636f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfigurationBundle

## Description
GetConfigurationBundle's not-found path is wrong in two independent ways: the 404 it does produce carries an emulator-invented sentence instead of AWS's, and after a successful DeleteConfigurationBundle the bundle is never removed from state, so the 404 never happens at all. 2 causes must both be fixed before this issue is closed.

Cause 1 of 2: GetConfigurationBundle on an unknown bundleId returns the wrong message text.
AWS: GetConfigurationBundle models ResourceNotFoundException (Smithy: GetConfigurationBundle errors include ResourceNotFoundException, httpError 404; http GET /configuration-bundles/{bundleId}, bundleId @httpLabel required, pattern ^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$). Recorded behavior: the 404 body is {"message": "Configuration bundle not found"} with the x-amzn-errortype error header. That message is entirely FIXED text — it has no variable parts at all: it does not interpolate the requested bundleId, the branchName, or the account. The requirement is that exact sentence.
Emulator: returns 404 with $.message = "Failed to retrieve configuration bundle because it doesn't exist. Retry the request with a different resource identifier." — the emulator's generic agentcore not-found template (the same house-style sentence it uses for gateways) applied to a resource family whose AWS wording is different and much shorter.
Why: a client or test that matches AWS's documented not-found text for configuration bundles (and retry/ignore logic keyed on it) never matches, so a benign "bundle absent" outcome is seen as an unrecognised error. This is the same class of defect as the open issue "Not-found and invalid-state error messages are emulator-invented text instead of AWS wording" (20261029T170911Z-bedrock-agentcore-control-not-found-and-invalid-state-error-messages-are.md), but that issue covers only the gateway, agent-runtime, harness and memory families and does not carry a configuration-bundle replay key, so this cause is not covered there.
Fix: the GetConfigurationBundle handler's not-found error construction must emit the configuration-bundle-specific fixed sentence rather than falling through to the shared agentcore resource-not-found template.
Traces: bedrock-agentcore-control-configuration-bundle-error-paths#task#1 (cluster also covers bedrock-agentcore-control-configuration-bundle-errors#task#2).

Cause 2 of 2: a deleted configuration bundle remains readable — DeleteConfigurationBundle does not remove it from state.
AWS: DeleteConfigurationBundle (Smithy: http DELETE /configuration-bundles/{bundleId}, code 202, idempotent, output bundleArn/bundleId/status:ConfigurationBundleStatus) accepts the delete and the bundle then stops existing, so a subsequent GetConfigurationBundle on that bundleId answers 404 ResourceNotFoundException with the fixed message "Configuration bundle not found" (same contract as cause 1). Recorded behavior: the fixture polls GetConfigurationBundle after the delete and the terminal observation is the 404 with that body.
Emulator: GetConfigurationBundle keeps answering 200 with the full bundle representation (bundleArn, bundleId, bundleName, description, versionId, components, lineageMetadata, createdAt, updatedAt) indefinitely — the replay polled for the capped 60s / 61 attempts and never saw anything but 200. Consequently $.message and the x-amzn-errortype header are absent because no error is produced at all.
Why: delete is not durable. A client cannot tell a live bundle from a deleted one, cleanup and teardown never converge, delete-then-recreate flows see a stale version/components payload, and anything that waits for the resource to disappear hangs until its own timeout.
Fix: the DeleteConfigurationBundle handler's state mutation — the bundle (and its version lineage) must actually be removed from, or tombstoned in, the configuration-bundle store that GetConfigurationBundle reads, so reads after the 202 take the not-found path.
Traces: bedrock-agentcore-control-configuration-bundle-lifecycle#teardown#9 (cluster also covers bedrock-agentcore-control-configuration-bundle-round-trip#task#4).

## Evidence
Cause 1 — fixture bedrock-agentcore-control-configuration-bundle-error-paths, step task#1, GetConfigurationBundle, status 404 vs 404 (status and error shape matched; only $.message diverged):
  expected $.message = "Configuration bundle not found"
  actual   $.message = "Failed to retrieve configuration bundle because it doesn't exist. Retry the request with a different resource identifier."
  Same cluster: bedrock-agentcore-control-configuration-bundle-errors#task#2.
  The expected message is 100% fixed text; no part of it comes from the request or state, so no recorded value needs substituting.

Cause 2 — fixture bedrock-agentcore-control-configuration-bundle-lifecycle, step teardown#9, GetConfigurationBundle, expected 404 but got 200:
  expected body {"message": "Configuration bundle not found"} plus the x-amzn-errortype header
  actual: 200 with the live bundle document (bundleArn/bundleId/bundleName/description/versionId/components/lineageMetadata/createdAt/updatedAt); $.message missing, x-amzn-errortype missing
  replay detail: "poll: status 200 not in [404] after 61 attempt(s)", poll capped at 60s
  Same cluster: bedrock-agentcore-control-configuration-bundle-round-trip#task#4.
  The bundle ids, ARNs, version ids and timestamps in the recorded payload are per-run values and are not part of the requirement; the requirement is that the read answers 404 with the fixed message once the delete has been accepted.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-agentcore-control-configuration-bundle-error-paths, step task#1 — GET /configuration-bundles/{bundleId} with a well-formed but unknown bundleId; read $.message of the 404 and compare it to the fixed text "Configuration bundle not found".
3. Cause 2: fixture bedrock-agentcore-control-configuration-bundle-lifecycle, step teardown#9 — CreateConfigurationBundle, then DELETE /configuration-bundles/{bundleId} (202), then poll GET /configuration-bundles/{bundleId}; the emulator keeps answering 200 with the bundle body instead of converging to 404 ResourceNotFoundException. Same effect via bedrock-agentcore-control-configuration-bundle-round-trip#task#4.
