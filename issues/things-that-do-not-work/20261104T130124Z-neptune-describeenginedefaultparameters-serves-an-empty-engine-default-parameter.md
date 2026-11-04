Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-npt-traj-engine-default-parameter-levels
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-npt-traj-engine-default-parameter-levels
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*]
Issue-Commit: 7cd635da05c853b3aee981acc4bc8b13c0b954d8

# Neptune DescribeEngineDefaultParameters serves an empty engine-default parameter catalog

> _Note: this issue was identified against the code base at commit `7cd635da05c853b3aee981acc4bc8b13c0b954d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEngineDefaultParameters

## Description
THIS ISSUE HAS 1 CAUSE.

Cause 1 of 1: DescribeEngineDefaultParameters answers 200 for a valid Neptune DB-parameter-group family but reports NO engine-default parameters at all, so the instance-level engine-default catalog is empty.

AWS: DescribeEngineDefaultParameters takes a required DBParameterGroupFamily and returns EngineDefaults (Smithy: com.amazonaws.neptune#DescribeEngineDefaultParameters, input DescribeEngineDefaultParametersMessage.DBParameterGroupFamily required, output DescribeEngineDefaultParametersResult.EngineDefaults; errors=[]; paginated items=EngineDefaults.Parameters, pageSize=MaxRecords). EngineDefaults.Parameters targets ParametersList, a LIST of com.amazonaws.neptune#Parameter, documented as 'Contains a list of engine default parameters'; each Parameter carries ParameterName, ParameterValue, Description, Source, ApplyType, DataType, AllowedValues and IsModifiable (Smithy: neptune#Parameter members). The model states no error for a published family, so the service answers 200 with the family's engine defaults populated. Recorded behavior only for the catalog contents: for the Neptune instance-level family the service reports the parameter neptune_query_timeout with Source 'engine-default', ApplyType 'static', DataType 'integer', AllowedValues '10-2147483647', IsModifiable 'true', Description 'Graph query timeout (ms).' (the description is fixed catalog text for that parameter, including its terminating period) and the family's default ParameterValue for it (the recording shows '120000'; the number is the catalog's default, not a per-run value).

Emulator: the response is HTTP 200 but EngineDefaults.Parameters has no element at index 0 — replay reports '$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[0]: missing (expected {...neptune_query_timeout...}, got None)'. The engine-defaults envelope is emitted while the parameter list it is supposed to carry is empty, so not one engine-default instance parameter is published.

Why: this is the discovery call a client makes to learn which instance-level parameters exist for a family, what their defaults are, which are modifiable and what values they accept, before calling CreateDBParameterGroup/ModifyDBParameterGroup. Against the emulator it concludes the family publishes zero parameters: it cannot pre-validate a ModifyDBParameterGroup request, cannot show an operator the engine default for neptune_query_timeout, and any configuration-drift check that compares a group against the engine defaults compares against an empty baseline and reports every parameter as unset rather than as default. A paginating client also terminates immediately on an empty items list.

Fix: the engine-default instance-parameter reference catalog read by the DescribeEngineDefaultParameters handler for the Neptune front door — the handler resolves the requested DBParameterGroupFamily to a catalog that has no instance-level entries for it and serializes the resulting empty list, instead of projecting the family's engine-default Parameter entries (name, value, source 'engine-default', apply type, data type, allowed values, modifiability and description) into EngineDefaults.Parameters.

Related open issue: 20261103T175309Z-neptune-parameter-and-engine-version-catalogs-serve-rds.md describes an earlier shape of this step (its cause 4 reported Parameters.Parameter arriving as a bare structure with an RDS-worded description). That is NOT this divergence: the current replay reports the member as absent rather than mis-shaped, and the cluster comes back with owned_by=[] and stub_path="", so no open issue carries its replay key.

Traces: neptune-npt-traj-engine-default-parameter-levels#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 19 fixtures replayed, 181 steps (110 passed, 69 mismatched, 2 blocked), score 60.8%, 45 divergence clusters — all service=neptune. 44 of them carry a non-empty owned_by; this one is the single cluster with owned_by=[] and stub_path="".

Cause 1 — fixture neptune-npt-traj-engine-default-parameter-levels, step task#2, operation DescribeEngineDefaultParameters, verdict mismatch, expected_status 200 / actual_status 200, affected_count 1.
divergence_key: service=neptune|DescribeEngineDefaultParameters|status=200->200|keys=$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[0]
detail: $.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[0]: missing (expected {'AllowedValues': '10-2147483647', 'ApplyType': 'static', 'DataType': 'integer', 'Description': 'Graph query timeout (ms).', 'Source': 'engine-default', 'ParameterName': 'neptune_query_timeout', 'IsModifiable': 'true', 'ParameterValue': '120000'}, got None).

Expected: HTTP 200 whose EngineDefaults.Parameters list carries the family's engine-default parameters, each a neptune#Parameter with ParameterName/ParameterValue/Description/Source/ApplyType/DataType/AllowedValues/IsModifiable populated.
Actual: HTTP 200 whose EngineDefaults.Parameters list is empty — no Parameter element is serialized.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) and locate the cluster for fixture neptune-npt-traj-engine-default-parameter-levels, step task#2.
3. That step calls DescribeEngineDefaultParameters against the Neptune endpoint with the instance-level DBParameterGroupFamily used by the fixture.
4. Inspect DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters: the emulator returns 200 with no Parameter element, where the recording carries at least the neptune_query_timeout engine default with Source 'engine-default', ApplyType 'static', DataType 'integer', AllowedValues '10-2147483647', IsModifiable 'true' and Description 'Graph query timeout (ms).'.
5. After the fix, re-replay neptune-npt-traj-engine-default-parameter-levels#task#2 and confirm the engine-default parameter list is populated for that family.
