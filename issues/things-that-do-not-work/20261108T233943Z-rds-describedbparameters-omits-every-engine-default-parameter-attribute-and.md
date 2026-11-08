Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-parameters-pagination
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-parameters-pagination
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].AllowedValues
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].ApplyType
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].DataType
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].Description
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].IsModifiable
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[*].MinimumEngineVersion
Issue-Commit: 0d0b5d4835067c793fae3be07582601671bbaf82

# DescribeDBParameters omits every engine-default parameter attribute and echoes unknown parameter names

> _Note: this issue was identified against the code base at commit `0d0b5d4835067c793fae3be07582601671bbaf82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBParameters

## Description
This bundle carries 3 net-new causes, one per paged DescribeDBParameters request in the rds-db-parameters-pagination fixture. They share ONE fault: the emulator's DB parameter-group state stores only what a client wrote (name, value, source, apply method) and has no engine-default parameter catalog behind it, so every descriptive attribute of a parameter is absent from the response.

AWS: DescribeDBParameters returns, for each parameter in the group, the full Parameter structure — ParameterName, ParameterValue, Source, ApplyMethod AND the catalog-derived attributes Description, ApplyType, DataType, AllowedValues, IsModifiable and MinimumEngineVersion. Smithy: DescribeDBParameters -> DBParameterGroupDetails.Parameters (list of Parameter); Parameter members include Description ('Provides a description of the parameter'), ApplyType ('Specifies the engine specific parameters type'), DataType ('Specifies the valid data type for the parameter'), AllowedValues ('Specifies the valid range of values for the parameter'), IsModifiable ('Indicates whether ... the parameter can be modified') and MinimumEngineVersion ('The earliest engine version to which the parameter can apply'). The operation is paginated (inputToken/outputToken Marker, items Parameters, pageSize MaxRecords), and every page of a real group is drawn from the engine's default parameter catalog for the group's family. Status is 200 in the recordings.

Emulator: 200, and each returned Parameter carries only ParameterName, ParameterValue, Source, ApplyMethod and SupportedEngineModes — AllowedValues, ApplyType, DataType, Description and IsModifiable are reported `missing` for every row on every page.

Why: these attributes are what a client uses to decide whether it may touch a parameter at all. A console or tuning tool that renders a parameter list, validates a candidate value against AllowedValues/DataType, hides non-modifiable parameters using IsModifiable, or decides whether a change needs a reboot from ApplyType gets nothing to work with, and code that dereferences those members against the emulator fails where it succeeds against AWS. Because the catalog is missing, paging the group also yields pages that are not the engine's default parameters at all.

Fix: the rds service crate's DB parameter-group state plus the DescribeDBParameters projection. Seed each group from an engine-default parameter catalog keyed by DBParameterGroupFamily, keep user writes as overlays with Source=user, and project the catalog attributes (Description, ApplyType, DataType, AllowedValues, IsModifiable, MinimumEngineVersion) on every row; page that catalog for MaxRecords/Marker.

Cause 1 of 3: first paged page of a mysql group — recorded rows carry e.g. auto_increment_increment with AllowedValues '1-65535', ApplyType 'dynamic', DataType 'integer', IsModifiable 'true' and a Description; all five attributes are missing on every row of the emulator's page (20 rows affected). Trace: rds-db-parameters-pagination#task#4.

Cause 2 of 3: the next page — recorded rows carry e.g. AllowedValues '0-9'/'0-75'/'10-100', ApplyType 'dynamic', DataType 'integer' and descriptions for the InnoDB compression parameters; same five attributes missing. Trace: rds-db-parameters-pagination#task#5.

Cause 3 of 3: a later page — recorded rows carry e.g. AllowedValues '0-65536' / '0-31536000' / '0-70' with descriptions ('The size of the internal host cache.', ...) plus MinimumEngineVersion on one row; same attributes missing. Trace: rds-db-parameters-pagination#task#8.

Related but NOT covering this: open issues 20261029T214632Z-rds-parameter-listings-omit-engine-default-metadata-and.md and 20261030T001542Z-rds-engine-parameter-and-event-category-catalogs-are.md own the DescribeDBClusterParameters clusters (including the one where an unknown name written by ModifyDBClusterParameterGroup is echoed back as an extra Parameter row). The three DescribeDBParameters causes above are reported unowned by the replay.

## Evidence
Replay of trace set simparity-wire-aws-v1. All three clusters: verdict=mismatch, expected_status=200, actual_status=200, divergence_key 'service=rds|DescribeDBParameters|status=200->200|keys=$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter[N].AllowedValues,...ApplyType,...DataType,...Description,...IsModifiable' repeated for every index on the page.

Cause 1 detail (excerpt): 'Parameter[0].AllowedValues: missing (expected 1-65535, got None); Parameter[0].ApplyType: missing (expected dynamic, got None); Parameter[0].DataType: missing (expected integer, got None); Parameter[0].Description: missing (expected "Intended for use with master-to-master replication, and can be used to control the operation of AUTO_INCREMENT columns", got None); Parameter[0].IsModifiable: missing (expected true, got None)' — and the same five keys for indices 1..19.

Cause 3 detail (excerpt): 'Parameter[0].Description: missing (expected "The size of the internal host cache.", got None)', 'Parameter[1].AllowedValues: missing (expected 0-31536000, got None)', plus Parameter[11].Description and indices 2..10.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-db-parameters-pagination step task#4 (DescribeDBParameters with MaxRecords on a mysql DB parameter group).
3. Inspect DescribeDBParametersResult.Parameters: each Parameter has ParameterName/ParameterValue/Source/ApplyMethod only — no AllowedValues, ApplyType, DataType, Description or IsModifiable.
4. Repeat for steps task#5 and task#8 of the same fixture (subsequent pages) — same omission on every row.
