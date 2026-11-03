Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|GetApplication|2xx->4xx|$.Id
Issue-Key: appconfig|GetApplication|2xx->4xx|$.Name
Issue-Key: appconfig|GetApplication|2xx->4xx|status
Issue-Key: appconfig|GetEnvironment|2xx->4xx|$.ApplicationId
Issue-Key: appconfig|GetEnvironment|2xx->4xx|$.Id
Issue-Key: appconfig|GetEnvironment|2xx->4xx|$.Name
Issue-Key: appconfig|GetEnvironment|2xx->4xx|$.State
Issue-Key: appconfig|GetEnvironment|2xx->4xx|status
Issue-Key: appconfig|GetConfigurationProfile|2xx->4xx|$.ApplicationId
Issue-Key: appconfig|GetConfigurationProfile|2xx->4xx|$.Id
Issue-Key: appconfig|GetConfigurationProfile|2xx->4xx|$.LocationUri
Issue-Key: appconfig|GetConfigurationProfile|2xx->4xx|$.Name
Issue-Key: appconfig|GetConfigurationProfile|2xx->4xx|$.Type
Issue-Key: appconfig|GetConfigurationProfile|2xx->4xx|status
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.ApplicationId
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.AppliedExtensions
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.CompletedAt
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.ConfigurationLocationUri
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.ConfigurationName
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.ConfigurationProfileId
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.ConfigurationVersion
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.DeploymentDurationInMinutes
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.DeploymentNumber
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.DeploymentStrategyId
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.EnvironmentId
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.EventLog
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.FinalBakeTimeInMinutes
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.GrowthFactor
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.GrowthType
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.PercentageComplete
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.StartedAt
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.State
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.Type
Issue-Key: appconfig|StartDeployment|2xx->4xx|$.VersionLabel
Issue-Key: appconfig|StartDeployment|2xx->4xx|status
Issue-Commit: eb95e56228ce55bd248c3581c6fd09ba0e6c8f48

# AppConfig control-plane reads do not resolve application, environment or configuration-profile by Name

> _Note: this issue was identified against the code base at commit `eb95e56228ce55bd248c3581c6fd09ba0e6c8f48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
This issue bundles 4 net-new causes, all one root cause: AppConfig's identifier resolution in this emulator only accepts the generated 7-character logical id, never the resource's Name. AWS AppConfig accepts EITHER the logical id OR the resource name in the `{ApplicationId}` / `{EnvironmentId}` / `{ConfigurationProfileId}` path labels and in the `ConfigurationProfileId` body member, which is why those members are modelled as free-form names (`Name`, length 1..64 / `LongName`, length 1..128) rather than as the id pattern `^[a-z0-9]{4,7}$` that the OUTPUT members carry. The emulator compares the supplied token only against the stored logical id, so every name-form request misses and is refused.

Cause 1 of 4: GetApplication with the application's Name.
  AWS: Smithy: GetApplication GET /applications/{ApplicationId}; input member ApplicationId targets `Name` (@length min 1 max 64) with NO id pattern, while output `Application.Id` targets `Id` (@pattern ^[a-z0-9]{4,7}$) — the input therefore accepts a name as well as an id. Recorded behavior: the same application fetched by Name answers 200 with the resolved `Id` and `Name` of that application.
  Emulator: answers 404 (ResourceNotFoundException) with no body members; `Id` and `Name` are absent.
  Why: a client that holds only the application name it created (the normal case for IaC and CLI users, since AWS lets you pass the name everywhere) cannot read its own application at all, and the 404 is indistinguishable from a deleted resource.
  Fix: the GetApplication handler's identifier lookup (and the shared application-resolution helper it calls in the appconfig service state) must resolve the label by logical id first and then by exact Name.
  Traces: appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name#task#7 (expected 200, got 404).

Cause 2 of 4: GetEnvironment with the application and environment Names.
  AWS: Smithy: GetEnvironment GET /applications/{ApplicationId}/environments/{EnvironmentId}; both labels target `Name` (@length 1..64), no id pattern; output Environment.{ApplicationId,Id} target `Id`. Recorded behavior: the by-name request answers 200 carrying the resolved ApplicationId, Id, Name and State (`ReadyForDeployment`).
  Emulator: 404, with ApplicationId, Id, Name and State all absent.
  Why: the same name-resolution gap as Cause 1, one level down — and because the environment also carries `State`, a client polling an environment it addresses by name can never observe readiness.
  Fix: the GetEnvironment handler's environment lookup, using the same id-then-Name resolution inside the application scope.
  Traces: appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name#task#9 (expected 200, got 404).

Cause 3 of 4: GetConfigurationProfile with the profile Name.
  AWS: Smithy: GetConfigurationProfile GET /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}; ConfigurationProfileId targets `LongName` (@length 1..128), no id pattern; output ConfigurationProfile.Id targets `Id`. Recorded behavior: by-name request answers 200 with the resolved ApplicationId, Id, Name, LocationUri (`hosted`) and Type (`AWS.Freeform`).
  Emulator: 404, all of those members absent.
  Why: a client cannot fetch a profile by the name it chose, so it cannot discover the profile's LocationUri/Type before deploying it.
  Fix: the GetConfigurationProfile handler's profile lookup (id-then-Name within the resolved application).
  Traces: appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name#task#11 (expected 200, got 404).

Cause 4 of 4: StartDeployment with ConfigurationProfileId given as the profile Name.
  AWS: Smithy: StartDeployment POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, http code 201; required body member ConfigurationProfileId targets `LongName` (@length 1..128) with NO id pattern, so the profile name is a legal value; the output `Deployment.ConfigurationProfileId` targets `Id` (^[a-z0-9]{4,7}$), i.e. AWS resolves the supplied name to the id on the way out. Recorded behavior: the name-form StartDeployment answers 201 with the full Deployment (ApplicationId, EnvironmentId, DeploymentStrategyId, ConfigurationProfileId resolved to the logical id, ConfigurationName, ConfigurationLocationUri, ConfigurationVersion, VersionLabel, DeploymentNumber, State COMPLETE, PercentageComplete 100.0, EventLog with DEPLOYMENT_COMPLETED then DEPLOYMENT_STARTED, AppliedExtensions []).
  Emulator: 404 ResourceNotFoundException; no deployment is created and the whole Deployment body is absent.
  Why: deploying by profile name is the documented and widely used form; refusing it makes the operation unusable for any caller that did not retain the generated profile id, and the 404 wrongly reports the profile as nonexistent.
  Fix: StartDeployment's ConfigurationProfileId resolution in its handler/validation, sharing the id-then-Name resolver, and the response serializer must emit the resolved logical id.
  Traces: appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted#task#10 (expected 201, got 404).

## Evidence
Cause 1 — appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name#task#7 GetApplication: expected status 200, actual 404; $.Id missing (expected 'ho65pb1'), $.Name missing (expected the application's name).
Cause 2 — same fixture task#9 GetEnvironment: expected 200, actual 404; $.ApplicationId, $.Id, $.Name, $.State ('ReadyForDeployment') all missing.
Cause 3 — same fixture task#11 GetConfigurationProfile: expected 200, actual 404; $.ApplicationId, $.Id, $.LocationUri ('hosted'), $.Name, $.Type ('AWS.Freeform') all missing.
Cause 4 — appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted#task#10 StartDeployment: expected 201, actual 404; the entire Deployment body (ApplicationId, EnvironmentId, DeploymentStrategyId, ConfigurationProfileId, ConfigurationName, ConfigurationLocationUri, ConfigurationVersion, VersionLabel, DeploymentNumber, DeploymentDurationInMinutes, FinalBakeTimeInMinutes, GrowthType, GrowthFactor, State 'COMPLETE', PercentageComplete 100.0, EventLog, AppliedExtensions [], StartedAt, CompletedAt, Type 'USER') is missing.
In every case the identical request issued with the generated logical id succeeds in the same fixture, which isolates the fault to name resolution rather than to the resource's existence.

## Steps to Reproduce
1. Create an AppConfig application, an environment under it and a hosted configuration profile, keeping both the returned logical ids and the names you supplied.
2. Call GetApplication with ApplicationId = the application NAME. AWS answers 200 with Id/Name; the emulator answers 404.
3. Call GetEnvironment with both labels as NAMES. AWS answers 200 with Id/Name/State; the emulator answers 404.
4. Call GetConfigurationProfile with ConfigurationProfileId = the profile NAME. AWS answers 200 with Id/Name/LocationUri/Type; the emulator answers 404.
5. Call StartDeployment with ConfigurationProfileId = the profile NAME (ids for the rest). AWS answers 201 and resolves ConfigurationProfileId to the logical id in the response; the emulator answers 404 and creates no deployment.
6. Replay appconfig-lacs-traj-control-plane-get-by-logical-id-vs-name (task#7, task#9, task#11) and appconfig-lacs-traj-redeploy-parameter-mistakes-then-profile-name-accepted (task#10).
