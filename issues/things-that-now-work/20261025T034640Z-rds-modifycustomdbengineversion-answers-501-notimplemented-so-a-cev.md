Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.custom-engine-version-core-attributes-immutable-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.custom-engine-version-core-attributes-immutable-rejected|run#22|AssertionError|assert code2 == "InvalidCustomDBEngineVersionStateFault", f"unexpected error code {code2!r}"
Issue-Commit: df6081fa6700f84ddafc058f223e27db3aea4084

# RDS ModifyCustomDBEngineVersion answers 501 NotImplemented, so a CEV's status/description can never be modified and the state refusal is never classified

> _Note: this issue was identified against the code base at commit `df6081fa6700f84ddafc058f223e27db3aea4084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCustomDBEngineVersion

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: ModifyCustomDBEngineVersion is still a scaffold stub while the rest of the custom-engine-version (CEV) family is implemented, so the one modify path a client has over a CEV answers 501 instead of either succeeding or returning the operation's modeled state refusal. Failing check: rds.custom-engine-version-core-attributes-immutable-rejected.

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): ModifyCustomDBEngineVersion takes Engine (CustomEngineName, required, len 1..35, pattern ^[A-Za-z0-9-]{1,35}$), EngineVersion (CustomEngineVersion, required, len 1..60, pattern ^[a-z0-9_.-]{1,60}$), optional Description (len 1..1000) and optional Status (enum CustomEngineVersionStatus: available | inactive | inactive-except-restore); it returns the DBEngineVersion shape. Its modeled errors are CustomDBEngineVersionNotFoundFault (HTTP 404, awsQuery code CustomDBEngineVersionNotFoundFault) and InvalidCustomDBEngineVersionStateFault (HTTP 400, awsQuery code InvalidCustomDBEngineVersionStateFault). The modeled Status documentation states the CEV "must not currently be in use" and that status changes are state-gated, which is the rule the check exercises: modifying even the mutable Description of a CEV that has not reached 'available' is refused 400 InvalidCustomDBEngineVersionStateFault, and the CEV's stored description is left untouched.

Emulator: the request is answered HTTP 501 with error code 'NotImplemented' (replay: "AssertionError: unexpected error code 'NotImplemented'"). The handler is an untouched scaffold: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_custom_db_engine_version.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its handle body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("ModifyCustomDBEngineVersion"))`. Note that the neighbouring CEV operations DO work in this build: the same check's CreateCustomDBEngineVersion create, its duplicate-identity refusal (CustomDBEngineVersionAlreadyExistsFault) and DescribeDBEngineVersions over the stored CEV all passed before the modify, so this is an isolated gap in one operation, not a missing resource family.

Why: a client that drives the documented CEV lifecycle (register media, wait for 'available', flip status to inactive / back to available, edit the description) cannot take any of those steps, and it cannot tell a state-gated refusal from an unimplemented endpoint: 501/'NotImplemented' is not a modeled error for the operation, so SDK error handling that branches on InvalidCustomDBEngineVersionStateFault (retry when the CEV settles) or CustomDBEngineVersionNotFoundFault (give up) takes neither branch and surfaces an unretryable server error instead.

Fix: implement the operation in its own handler file, services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_custom_db_engine_version.rs, over the existing CEV records in crate::state::custom_engine_versions (resolve (Engine, EngineVersion) in the request's region, 404 CustomDBEngineVersionNotFoundFault when it names nothing, 400 InvalidCustomDBEngineVersionStateFault while the CEV is not in a modifiable state, otherwise apply Description/Status and answer the stored CEV projected onto the DBEngineVersion shape), then delete the SCAFFOLDED marker line.

Traces: rds.custom-engine-version-core-attributes-immutable-rejected (assertion at line 138 of the check: code2 == "InvalidCustomDBEngineVersionStateFault").

## Evidence
Expected (AWS / Smithy): HTTP 400, error code InvalidCustomDBEngineVersionStateFault, for ModifyCustomDBEngineVersion(Engine='custom-oracle-ee', EngineVersion='19.<unique>', Description='...') against a CEV that has not reached 'available'; the CEV's DBEngineVersionDescription is unchanged afterwards.

Actual (emulator): HTTP 501, error code 'NotImplemented'. Replay detail: `File "check_rds_custom_engine_version_core_attributes_immutable_rejected.py", line 138, in run / assert code2 == "InvalidCustomDBEngineVersionStateFault", f"unexpected error code {code2!r}" / AssertionError: unexpected error code 'NotImplemented'`.

Source: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_custom_db_engine_version.rs:4 (`SCAFFOLDED, NOT IMPLEMENTED` marker) and :272 (`Err(crate::scaffolded::Scaffolded::not_implemented("ModifyCustomDBEngineVersion"))`). No open issue names this operation (the filed rds stub issues cover CopyOptionGroup, CreateDBParameterGroup, CreateDBSecurityGroup, DeleteDBSubnetGroup, DescribeAccountAttributes, DescribeEngineDefaultClusterParameters, ModifyEventSubscription, DescribeDBParameters, ModifyDBShardGroup and CreateDBCluster).

## Steps to Reproduce
1. Boot the emulator and point an RDS client at it.
2. CreateCustomDBEngineVersion(Engine='custom-oracle-ee', EngineVersion='19.<unique>', DatabaseInstallationFilesS3BucketName=<bucket>, DatabaseInstallationFilesS3Prefix='cev-media/', KMSKeyId=<key>, Manifest='{"mediaImportTemplateVersion":"2020-08-14","databaseInstallationFileNames":["V000000-01.zip"]}', Description='original') — this succeeds today and the CEV is stored in a non-'available' status.
3. Call ModifyCustomDBEngineVersion(Engine=<same>, EngineVersion=<same>, Description='attempted mutation while not available').
4. Observe HTTP 501 / error code 'NotImplemented' instead of HTTP 400 / InvalidCustomDBEngineVersionStateFault.
5. Replay the behavioral check: rds.custom-engine-version-core-attributes-immutable-rejected.
