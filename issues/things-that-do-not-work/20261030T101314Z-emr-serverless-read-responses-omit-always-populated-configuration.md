Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-error-paths
Issue-Step: teardown#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-error-paths emr-serverless-session-endpoint-and-resource-dashboard
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.autoStartConfig
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.autoStopConfig
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.autoStopConfiguration.idleTimeout
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.extendedSupportConfiguration
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.jobLevelCostAllocationConfiguration.enabled
Issue-Key: emr-serverless|GetApplication|2xx->2xx|$.application.monitoringConfiguration.managedPersistenceMonitoringConfiguration
Issue-Key: emr-serverless|GetSession|2xx->2xx|$.session.extendedSupportEnabled
Issue-Key: emr-serverless|GetSessionEndpoint|2xx->2xx|$.endpoint
Issue-Commit: e63ec425c23513ba26a0bb6d325f047894e211cc

# EMR Serverless read responses omit always-populated configuration blocks and serialize a non-AWS session endpoint

> _Note: this issue was identified against the code base at commit `e63ec425c23513ba26a0bb6d325f047894e211cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
The emr-serverless read-path serializers emit only the fields the emulator happens to hold in state: configuration blocks that AWS always materialises (with their service defaults) are absent from the response, and the one derived URL field it does emit is built from the emulator's own listener address instead of the AWS session-endpoint form. All three causes are the same defect class — the response projection for a read operation is not the projection AWS returns — in one service. 3 causes.

Cause 1 of 3: GetApplication omits the application's effective configuration blocks.
AWS: GetApplication returns the required `application` member of the Application structure (Smithy: GetApplication output application: Application @required; Application members include autoStartConfiguration: AutoStartConfig, autoStopConfiguration: AutoStopConfig {enabled: Boolean, idleTimeoutMinutes: Integer range 1..10080}, monitoringConfiguration: MonitoringConfiguration {managedPersistenceMonitoringConfiguration: ManagedPersistenceMonitoringConfiguration {enabled: Boolean, encryptionKeyArn}}, jobLevelCostAllocationConfiguration: JobLevelCostAllocationConfiguration {enabled: Boolean}). The documented defaults are the values AWS fills in when the client supplied none: auto-start enabled defaults to true, auto-stop enabled defaults to true with a 15-minute idle timeout, managed log persistence enabled defaults to true. The recording additionally shows AWS emitting keys the model does not declare — `autoStartConfig`, `autoStopConfig` (enabled plus both the idleTimeout and idleTimeoutMinutes spellings), an `extendedSupportConfiguration` block carrying enabled/source/updatedAt/updatedBy, and `autoStopConfiguration.idleTimeout` alongside idleTimeoutMinutes — so for those keys this is recorded behavior only: AWS populates the whole effective-configuration projection, defaults included, for an application created without those options. Values in the recording that are booleans/defaults (auto-start enabled, auto-stop enabled, managed persistence enabled, cost allocation disabled, extended support source ACCOUNT with null updatedAt/updatedBy) are the service defaults, not per-run data; the idle timeout is whatever the application was created with, defaulting to 15.
Emulator: every one of those members comes back absent (null) on the same GetApplication call — autoStartConfig, autoStopConfig, autoStopConfiguration.idleTimeout, extendedSupportConfiguration, jobLevelCostAllocationConfiguration.enabled and monitoringConfiguration.managedPersistenceMonitoringConfiguration are all missing, while the status is correctly 200.
Why: a client that reads the effective configuration back after create — to confirm auto-stop is on, to read the idle timeout it will have to wait out, or to check whether managed log persistence is enabled before looking for logs — sees null and must either guess the defaults or treat the application as misconfigured. Console-style and IaC-drift callers that diff the returned configuration against the desired one see spurious drift on every field.
Fix: the GetApplication response projection/serializer plus the application record's default materialisation in the emr-serverless service crate's state — apply the documented defaults at create time and serialize the full configuration projection (both the modelled and the recorded alias spellings) on read.
Traces: emr-serverless-error-paths#teardown#9.

Cause 2 of 3: GetSession omits the session's extended-support flag.
AWS: GetSession returns the required `session` member of the Session structure (Smithy: GetSession output session: Session @required). The model does not declare an extendedSupportEnabled member, so this is recorded behavior only: the recorded 200 response carries $.session.extendedSupportEnabled as a boolean (false for a session started without extended support), i.e. AWS always emits the flag rather than omitting it.
Emulator: the member is absent from the session projection entirely (missing/null) on an otherwise correct 200 GetSession.
Why: a client reading the flag cannot distinguish "extended support off" from "field not returned", so a strongly typed consumer that requires the boolean fails to deserialize or silently defaults it the wrong way.
Fix: the GetSession response projection/serializer and the session record in the emr-serverless service crate's state — carry the extended-support flag on the session and always serialize it.
Traces: emr-serverless-session-endpoint-and-resource-dashboard#task#6.

Cause 3 of 3: GetSessionEndpoint returns the emulator's own listener URL instead of an EMR Serverless session-endpoint URL.
AWS: GetSessionEndpoint output endpoint is @required, target EndpointUrl, a string of @length 1..256 (Smithy: GetSessionEndpoint output {applicationId, sessionId, endpoint: EndpointUrl @required, authToken @required @sensitive, authTokenExpiresAt @required}). The recording shows the value is an https origin whose host is derived from the session identity and the region — scheme https, a host built from the session id plus the EMR Serverless session-endpoint domain for the request's region, and no path component. The host's literal id and region come from the request and the emulator's state; the required shape is "https origin naming the session under the regional emr-serverless session-endpoint domain, no path".
Emulator: it returns an http(s) URL pointing at its own data listener with a resource path appended — scheme/host/port of the local emulator process plus /applications/{applicationId}/sessions/{sessionId}/connect — i.e. a different URL form, not merely a different host. (The literal loopback host and port of any one run are environment-specific and are not the requirement; the divergence is that the value is a path-style local URL rather than the AWS session-endpoint origin.)
Why: a client that parses this value to extract the session host (to attach a Livy/Spark connect client, to build an SNI/Host header, or to validate the endpoint belongs to the session) gets a path where it expects a bare origin and a host that does not encode the session, so endpoint construction from the response fails.
Fix: the GetSessionEndpoint handler's endpoint-URL construction in the emr-serverless service crate — derive the endpoint as an origin from the session id and the configured region, keeping the configurable host substitution in one place rather than leaking the data listener's address and request path into the field.
Traces: emr-serverless-session-endpoint-and-resource-dashboard#task#7.

The open issue 20261029T182536Z-emr-serverless-autostopconfiguration-never-materialises-stopped-within-an concerns auto-stop not transitioning an application to STOPPED; it does not cover the configuration blocks being absent from GetApplication's response, nor the session projections here.

## Evidence
Cause 1 — fixture emr-serverless-error-paths, step teardown#9, GetApplication, status 200 both sides:
  $.application.autoStartConfig: missing (expected {enabled: true})
  $.application.autoStopConfig: missing (expected {enabled: true, idleTimeout: <configured minutes, default 15>, idleTimeoutMinutes: <same>})
  $.application.autoStopConfiguration.idleTimeout: missing (expected the configured idle timeout, default 15)
  $.application.extendedSupportConfiguration: missing (expected {enabled: false, source: "ACCOUNT", updatedAt: null, updatedBy: null})
  $.application.jobLevelCostAllocationConfiguration.enabled: missing (expected false)
  $.application.monitoringConfiguration.managedPersistenceMonitoringConfiguration: missing (expected {enabled: true, encryptionKeyArn: null})

Cause 2 — fixture emr-serverless-session-endpoint-and-resource-dashboard, step task#6, GetSession, status 200 both sides:
  $.session.extendedSupportEnabled: missing (expected the boolean flag, false for a session started without extended support)

Cause 3 — fixture emr-serverless-session-endpoint-and-resource-dashboard, step task#7, GetSessionEndpoint, status 200 both sides:
  $.endpoint: value_mismatch — expected an https origin of the form https://<sessionId-derived-host>.s.emr-serverless-services.<region>.amazonaws.com (no path); actual a local path-style URL of the form https://<emulator-host>:<port>/applications/{applicationId}/sessions/{sessionId}/connect

Model grounding: Smithy Application.autoStopConfiguration: AutoStopConfig (enabled defaults true, idleTimeoutMinutes defaults 15, range 1..10080); Application.monitoringConfiguration.managedPersistenceMonitoringConfiguration.enabled defaults true; Application.jobLevelCostAllocationConfiguration.enabled: Boolean; GetSessionEndpoint.endpoint: EndpointUrl @required @length 1..256. extendedSupportEnabled / extendedSupportConfiguration / the autoStartConfig+autoStopConfig alias keys are not in the on-disk model — recorded behavior only.

## Steps to Reproduce
1. Replay fixture emr-serverless-error-paths; at teardown#9 call GetApplication (GET /applications/{applicationId}) for an application created without auto-start/auto-stop/monitoring options. Observe that autoStartConfig, autoStopConfig, autoStopConfiguration.idleTimeout, extendedSupportConfiguration, jobLevelCostAllocationConfiguration.enabled and monitoringConfiguration.managedPersistenceMonitoringConfiguration are all absent from the 200 response.
2. Replay fixture emr-serverless-session-endpoint-and-resource-dashboard; at task#6 call GetSession (GET /applications/{applicationId}/sessions/{sessionId}) and observe $.session.extendedSupportEnabled is absent.
3. In the same fixture, at task#7 call GetSessionEndpoint (GET /applications/{applicationId}/sessions/{sessionId}/endpoint) and observe $.endpoint is the emulator's own listener URL with an /applications/.../connect path instead of an EMR Serverless session-endpoint origin.
