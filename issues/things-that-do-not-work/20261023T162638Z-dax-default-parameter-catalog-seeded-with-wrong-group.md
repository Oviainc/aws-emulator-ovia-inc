Issue-Source: trace-sweep-divergence
Issue-Trace: dax-catalog-describe-default-parameters
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dax-catalog-describe-default-parameters dax-parameter-group-lifecycle dax-catalog-describe-parameter-groups
Issue-Shape-Id: com.amazonaws.dax#AmazonDAXV3
Issue-Key: dax|DescribeDefaultParameters|2xx->2xx|$.Parameters[*]
Issue-Key: dax|DescribeParameterGroups|2xx->2xx|$.ParameterGroups[*]
Issue-Commit: c5ab1277cf41f7c1d08660bb7be869a7fe462875

# DAX default parameter catalog seeded with wrong group name, description and parameter Source

> _Note: this issue was identified against the code base at commit `c5ab1277cf41f7c1d08660bb7be869a7fe462875`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDefaultParameters

## Description
This bundle carries 2 causes, both the same fault: the account's pre-existing DAX parameter catalog is seeded with values the recorded service does not use, so every read of it diverges even though the response shape and HTTP 200 are correct.

Cause 1 of 2: DescribeDefaultParameters reports Parameter$Source 'system' and a non-matching description for record-ttl-millis (fixture dax-catalog-describe-default-parameters, step task#1; the same cluster also covers dax-parameter-group-lifecycle#task#4).
  AWS: HTTP 200 with Parameters for query-ttl-millis and record-ttl-millis. Recorded behavior only -- the model does not pin these values (Smithy: DescribeDefaultParameters output Parameters: ParameterList; Parameter$Source doc is only 'How the parameter is defined. For example, system denotes a system-defined parameter.', i.e. prose, not a constraint). The recording shows Source 'user' on both default parameters, and record-ttl-millis described as 'Duration in milliseconds for records to remain valid in cache (Default: 0 = infinite)'.
  Emulator: HTTP 200 with the same two parameter names and values, but Source 'system' on both and record-ttl-millis described as 'Duration in milliseconds for records to remain cached'. All other members (ParameterName, ParameterType DEFAULT, ParameterValue 300000, DataType integer, AllowedValues '0-', IsModifiable TRUE, ChangeType IMMEDIATE, empty NodeTypeSpecificValues) match.
  Why: a client that filters or displays parameters by Source (the usual way to tell an overridden setting from a service-defined one) classifies every DAX default parameter the opposite way from the real service, and a config-diffing or drift-detection tool sees a change where there is none.
  Fix: the seeded parameter catalog in the service's state -- the Source constant used when projecting a default parameter, and the record-ttl-millis description string.
  Traces: dax-catalog-describe-default-parameters#task#1; dax-parameter-group-lifecycle#task#4. Code: services/service-com-amazonaws-dax-amazondaxv3/src/state.rs (PARAMETER_SOURCE_SYSTEM ~line 575, DEFAULT_PARAMETERS ~line 659).

Cause 2 of 2: DescribeParameterGroups lists the built-in group as 'default.dax' instead of the versioned name (fixture dax-catalog-describe-parameter-groups, step task#1).
  AWS: HTTP 200 with one pre-existing ParameterGroup whose ParameterGroupName is version-qualified -- recorded as 'default.dax1.0' with Description 'Default parameter group for dax1.0'. Recorded behavior only (Smithy: ParameterGroup has free-form ParameterGroupName/Description, no default pinned in the model), but the name is the identifier clients pass back on the wire, not display text.
  Emulator: HTTP 200 with ParameterGroupName 'default.dax' and Description 'Default parameter group.'.
  Why: the default parameter group's name is a request value -- CreateCluster$ParameterGroupName, UpdateCluster$ParameterGroupName, DescribeParameterGroups$ParameterGroupNames and DescribeParameters all take it. A client or template that names the real default, 'default.dax1.0', gets ParameterGroupNotFoundFault from this emulator, and a client that reads the name back from this emulator sends one AWS does not have.
  Fix: the default parameter group identity in the service's state (the reserved default name and its description), and anything that resolves that reserved name on create/delete.
  Traces: dax-catalog-describe-parameter-groups#task#1. Code: services/service-com-amazonaws-dax-amazondaxv3/src/state.rs (DEFAULT_PARAMETER_GROUP ~line 132, DEFAULT_PARAMETER_GROUP_DESCRIPTION ~line 135).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up):
- dax-catalog-describe-default-parameters#task#1 (also dax-parameter-group-lifecycle#task#4), DescribeDefaultParameters, status 200 vs 200: $.Parameters[0] expected Source 'user', got 'system'; $.Parameters[1] expected Source 'user' and Description 'Duration in milliseconds for records to remain valid in cache (Default: 0 = infinite)', got Source 'system' and Description 'Duration in milliseconds for records to remain cached'.
- dax-catalog-describe-parameter-groups#task#1, DescribeParameterGroups, status 200 vs 200: $.ParameterGroups[0] expected {ParameterGroupName 'default.dax1.0', Description 'Default parameter group for dax1.0'}, got {ParameterGroupName 'default.dax', Description 'Default parameter group.'}.
Source in the checkout: state.rs defines DEFAULT_PARAMETER_GROUP = "default.dax", DEFAULT_PARAMETER_GROUP_DESCRIPTION = "Default parameter group.", and projects every default parameter with PARAMETER_SOURCE_SYSTEM = "system".

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture dax-catalog-describe-default-parameters, step task#1: call DescribeDefaultParameters on a fresh account -> both returned Parameters carry Source 'system' (AWS: 'user'), and record-ttl-millis carries the shortened description.
3. Replay fixture dax-catalog-describe-parameter-groups, step task#1: call DescribeParameterGroups with no filter on a fresh account -> the single pre-existing group is named 'default.dax' (AWS: 'default.dax1.0').
4. Confirm the knock-on: CreateCluster or DescribeParameterGroups naming 'default.dax1.0' is rejected as not found by this emulator.
