Issue-Source: trace-sweep-divergence
Issue-Trace: launch-wizard-create-deployment-validation-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: launch-wizard-create-deployment-validation-errors launch-wizard-workload-catalog-discovery
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launchwizard|CreateDeployment|4xx->4xx|$.message
Issue-Key: launchwizard|CreateDeployment|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: launchwizard|CreateDeployment|4xx->4xx|status
Issue-Key: launchwizard|GetWorkload|2xx->2xx|$.workload.accountConstraints
Issue-Key: launchwizard|ListWorkloadDeploymentPatterns|2xx->2xx|$.workloadDeploymentPatterns[*]
Issue-Key: launchwizard|GetWorkloadDeploymentPattern|2xx->4xx|$.workloadDeploymentPattern
Issue-Key: launchwizard|GetWorkloadDeploymentPattern|2xx->4xx|status
Issue-Key: launchwizard|ListDeploymentPatternVersions|2xx->4xx|$.deploymentPatternVersions
Issue-Key: launchwizard|ListDeploymentPatternVersions|2xx->4xx|$.nextToken
Issue-Key: launchwizard|ListDeploymentPatternVersions|2xx->4xx|status
Issue-Key: launchwizard|GetDeploymentPatternVersion|2xx->4xx|$.deploymentPatternVersion
Issue-Key: launchwizard|GetDeploymentPatternVersion|2xx->4xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Seeded workload catalog is missing real AWS deployment patterns, so launchwizard pattern lookups answer 404 and CreateDeployment never validates

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkloadDeploymentPattern

## Description
Every per-pattern Launch Wizard read resolves against a placeholder catalog that does not contain the AWS deployment patterns, so lookups by a real pattern name or version answer ResourceNotFoundException, the pattern listing returns substitute items, accountConstraints is dropped from the serialized catalog data, and CreateDeployment's not-found path pre-empts the input validation that should have produced the response. 6 causes.

--- Cause 1 of 6: CreateDeployment answers 404 ResourceNotFoundException instead of 400 ValidationException aggregating the specification violations (fixture launch-wizard-create-deployment-validation-errors, step task#3) ---
AWS: Smithy: CreateDeployment, POST /createDeployment, success 200; required input workloadName, deploymentPatternName, name (DeploymentName) and specifications (DeploymentSpecifications map, @length 1..100, @sensitive); modeled errors ValidationException (client, httpError 400, x-amzn-errortype ValidationException), ResourceNotFoundException (client, 404) and ResourceLimitException (400). For an existing workload/pattern whose specifications map is wrong, the service validates the request and answers 400 ValidationException with ONE message that concatenates a clause per violation, comma-space separated, in this order: an unknown-key clause "Specification <key> is not allowed."; a member-pattern clause "The value of '<memberName>' doesn't match the regular expression pattern <'<pattern>'>."; then one "Missing specification <SpecificationName>." per required specification of that deployment pattern absent from the request. The fixed text is those three templates; the variable parts are the rejected specification key, the member name, the pattern, and each missing specification's name, all taken from the request and from the pattern's specification list. Recorded behavior only: the message quotes the deploymentName pattern as <'^[a-zA-Z][a-zA-Z0-9]+$'>, which is stricter than the on-disk DeploymentName @pattern ^[A-Za-z0-9_\.-]+$ - the message must quote the rule the service enforces for this error class, not the laxer member pattern.
Emulator: HTTP 404, x-amzn-errortype ResourceNotFoundException, message "The specified deployment pattern does not exist or insufficient permissions." The requested pattern is not in the placeholder catalog, so the handler returns not-found before any input validation runs.
Why: a client cannot learn what is wrong with its request. It sees a 404 about a pattern it just discovered instead of a 400 enumerating the unknown key, the malformed deployment name and every required specification it still has to supply - so it cannot self-correct, and SDK error branching keyed on ValidationException vs ResourceNotFoundException takes the wrong arm.
Fix: CreateDeployment's input validation in the launchwizard crate must run against the deployment pattern's specification list and emit the aggregated ValidationException, and the pattern lookup it depends on must resolve real catalog patterns (the shared catalog seed/state below) so that 404 is reserved for genuinely unknown workload/pattern pairs.
Traces: launch-wizard-create-deployment-validation-errors#task#3.

--- Cause 2 of 6: GetWorkload omits accountConstraints instead of serializing an empty list (fixture launch-wizard-workload-catalog-discovery, step task#2) ---
AWS: Smithy: GetWorkload, POST /getWorkload, success 200, output member workload: WorkloadData, whose members are workloadName, displayName, status, accountConstraints (AccountConstraintsList, @length 0..10, @uniqueItems), description, documentationUrl, iconUrl, statusMessage. The recording shows accountConstraints present and serialized as an empty JSON array for a workload with no account constraints.
Emulator: 200, but $.workload.accountConstraints is absent from the body entirely (expected [], got None).
Why: accountConstraints is how a client decides whether the calling account may deploy a workload. An absent member is not the same as an empty list: code that reads the field without a null guard faults, and code that treats absence as "unknown" shows a false constraint warning.
Fix: the GetWorkload serializer in the launchwizard crate - emit accountConstraints as an empty list rather than skipping the member when the catalog entry has none.
Traces: launch-wizard-workload-catalog-discovery#task#2.

--- Cause 3 of 6: ListWorkloadDeploymentPatterns returns substitute patterns at the wrong versions (fixture launch-wizard-workload-catalog-discovery, step task#3) ---
AWS: Smithy: ListWorkloadDeploymentPatterns, POST /listWorkloadDeploymentPatterns, success 200; output workloadDeploymentPatterns: list<WorkloadDeploymentPatternDataSummary>. For the SAP workload the recording's first element is the SAP HANA high-availability pattern at the workload's catalog deploymentPatternVersionName, status ACTIVE.
Emulator: 200 with a different first element - a different deploymentPatternName and a workloadVersionName/deploymentPatternVersionName pair taken from the placeholder seed rather than the catalog.
Why: the names and versions returned here are the keys for GetWorkloadDeploymentPattern, ListDeploymentPatternVersions, GetDeploymentPatternVersion and CreateDeployment. Returning names that do not exist in AWS means a client that round-trips its own discovery output still cannot address the pattern it wants.
Fix: the shared workload/deployment-pattern catalog seed/state in the launchwizard crate.
Traces: launch-wizard-workload-catalog-discovery#task#3.

--- Cause 4 of 6: GetWorkloadDeploymentPattern answers 404 for a real workload/pattern pair (fixture launch-wizard-workload-catalog-discovery, step task#4) ---
AWS: Smithy: GetWorkloadDeploymentPattern, POST /getWorkloadDeploymentPattern, success 200; required workloadName and deploymentPatternName; output workloadDeploymentPattern: WorkloadDeploymentPatternData; errors ResourceNotFoundException (404) and ValidationException (400). For an existing pair the service answers 200 with the pattern's deploymentPatternName, deploymentPatternVersionName, displayName, description, status, accountConstraints (empty list when unconstrained) and the full specifications list, each specification carrying name, description, required ("Yes"/"No"), optional allowedValues, and optional conditionals entries of {name, comparator, value}.
Emulator: HTTP 404, $.workloadDeploymentPattern absent - the pair is not in the placeholder catalog, so the handler takes its not-found path.
Why: the specifications list is the only machine-readable source of which deployment inputs exist, which are required and which are conditional. Without it a client cannot build a valid CreateDeployment request at all, which is exactly the failure Cause 1 then reports as a 404.
Fix: the shared catalog seed/state plus the GetWorkloadDeploymentPattern handler's not-found path.
Traces: launch-wizard-workload-catalog-discovery#task#4.

--- Cause 5 of 6: ListDeploymentPatternVersions answers 404 instead of a 200 page (fixture launch-wizard-workload-catalog-discovery, step task#5) ---
AWS: Smithy: ListDeploymentPatternVersions, POST /listDeploymentPatternVersions, success 200; required workloadName and deploymentPatternName, optional maxResults/nextToken/filters; output deploymentPatternVersions and nextToken (@length 1..2048); paginated (items=deploymentPatternVersions). For an existing pair the service answers 200 with the deploymentPatternVersions member present and an opaque nextToken when a further page exists; 404 ResourceNotFoundException is for a workload/pattern that does not exist.
Emulator: HTTP 404 with neither $.deploymentPatternVersions nor $.nextToken present (expected the list member and a continuation token, got None for both).
Why: a paginating client raises on page one rather than iterating, and never reaches the version it needs to pass to GetDeploymentPatternVersion or UpdateDeployment.
Fix: the shared catalog seed/state and this handler's not-found path. Distinct from open issue 20261029T045748Z (nextToken cursor scoping) and 20261028T114957Z (filter-value pattern in the error text): both of those presuppose a 200 this call never reaches.
Traces: launch-wizard-workload-catalog-discovery#task#5.

--- Cause 6 of 6: GetDeploymentPatternVersion answers 404 for a real version (fixture launch-wizard-workload-catalog-discovery, step task#6) ---
AWS: Smithy: GetDeploymentPatternVersion, POST /getDeploymentPatternVersion, success 200; required workloadName, deploymentPatternName and deploymentPatternVersionName (@length 5..30, @pattern ^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$); its only modeled client error is ResourceNotFoundException (404). For an existing triple the service answers 200 with deploymentPatternVersion carrying workloadName, deploymentPatternName, deploymentPatternVersionName and description; recorded behavior only - the recording also carries a featureFlags object whose members (cloudFormationOutput with a nested configuration.fields list, modifyDeployments, postDeploymentSteps, each with a status of DISABLED/ENABLED, and an updateFromVersion list) are not in the on-disk DeploymentPatternVersionDataSummary.
Emulator: HTTP 404, $.deploymentPatternVersion absent.
Why: this is the call that tells a client whether modification, post-deployment steps and CloudFormation output are available for a version, and which versions it can upgrade from. A 404 makes every version-aware flow unusable.
Fix: the shared catalog seed/state, this handler's not-found path, and the serializer for the version's featureFlags payload.
Traces: launch-wizard-workload-catalog-discovery#task#6.

## Evidence
Cause 1 - launch-wizard-create-deployment-validation-errors#task#3: expected status 400, got 404; $headers.x-amzn-errortype expected 'ValidationException', got 'ResourceNotFoundException'; $.message expected the aggregated clause list "Specification totally-invalid-spec-key is not allowed., The value of 'deploymentName' doesn't match the regular expression pattern <'^[a-zA-Z][a-zA-Z0-9]+$'>., Missing specification DisableDeploymentRollback., Missing specification SaveDeploymentArtifacts., Missing specification KeyPairName., Missing specification VpcId., Missing specification CreateSecurityGroup., Missing specification Timezone., Missing specification EnableEbsVolumeEncryption., Missing specification SapSysGroupId., Missing specification DatabaseSystemId., Missing specification DatabaseInstanceNumber., Missing specification InstallDatabaseSoftware., Missing specification DatabasePrimaryHostname., Missing specification DatabaseSecondaryHostname., Missing specification DatabasePrimarySiteName., Missing specification DatabaseSecondarySiteName., Missing specification DatabaseVirtualIpAddress., Missing specification DatabasePacemakerTag.", got 'The specified deployment pattern does not exist or insufficient permissions.' (the specification key, member name, pattern and missing-specification names are the request-/catalog-derived variable parts of those templates).
Cause 2 - launch-wizard-workload-catalog-discovery#task#2 (GetWorkload): status 200 both sides; $.workload.accountConstraints expected [], got None.
Cause 3 - launch-wizard-workload-catalog-discovery#task#3 (ListWorkloadDeploymentPatterns): status 200 both sides; $.workloadDeploymentPatterns[0] expected {workloadName: SAP, deploymentPatternName: SapHanaHA, deploymentPatternVersionName: 1.0.27, status: ACTIVE}, got {workloadName: SAP, deploymentPatternName: SapHanaMultiNode, workloadVersionName: 1.0.0, deploymentPatternVersionName: 2.0.0, ...}.
Cause 4 - launch-wizard-workload-catalog-discovery#task#4 (GetWorkloadDeploymentPattern): expected 200, got 404; $.workloadDeploymentPattern expected the SapHanaHA pattern with accountConstraints [] and a specifications list of {name, description, required, allowedValues, conditionals:[{name, comparator, value}]} entries, got None.
Cause 5 - launch-wizard-workload-catalog-discovery#task#5 (ListDeploymentPatternVersions): expected 200, got 404; $.deploymentPatternVersions expected present, got None; $.nextToken expected an opaque continuation token, got None.
Cause 6 - launch-wizard-workload-catalog-discovery#task#6 (GetDeploymentPatternVersion): expected 200, got 404; $.deploymentPatternVersion expected {workloadName: SAP, deploymentPatternName: SapHanaHA, deploymentPatternVersionName: 1.0.27, description, featureFlags:{cloudFormationOutput:{configuration:{fields:[]}, status: DISABLED}, modifyDeployments:{status: DISABLED}, postDeploymentSteps:{status: DISABLED}, updateFromVersion: []}}, got None.

## Steps to Reproduce
1. Boot the emulator from mainline and point a Launch Wizard client at its data listener.
2. Cause 2: POST /getWorkload {"workloadName":"SAP"} -> 200 but no accountConstraints member. Replay launch-wizard-workload-catalog-discovery#task#2.
3. Cause 3: POST /listWorkloadDeploymentPatterns {"workloadName":"SAP"} -> 200 with substitute pattern names/versions. Replay launch-wizard-workload-catalog-discovery#task#3.
4. Cause 4: POST /getWorkloadDeploymentPattern {"workloadName":"SAP","deploymentPatternName":"SapHanaHA"} -> 404 ResourceNotFoundException instead of 200 with the specifications list. Replay launch-wizard-workload-catalog-discovery#task#4.
5. Cause 5: POST /listDeploymentPatternVersions {"workloadName":"SAP","deploymentPatternName":"SapHanaHA"} -> 404 instead of a 200 page with nextToken. Replay launch-wizard-workload-catalog-discovery#task#5.
6. Cause 6: POST /getDeploymentPatternVersion {"workloadName":"SAP","deploymentPatternName":"SapHanaHA","deploymentPatternVersionName":"1.0.27"} -> 404 instead of 200 with featureFlags. Replay launch-wizard-workload-catalog-discovery#task#6.
7. Cause 1: POST /createDeployment for that workload/pattern with an unknown specification key, a deploymentName that violates the enforced pattern, and the required specifications omitted -> 404 ResourceNotFoundException instead of 400 ValidationException with the aggregated clause list. Replay launch-wizard-create-deployment-validation-errors#task#3.
