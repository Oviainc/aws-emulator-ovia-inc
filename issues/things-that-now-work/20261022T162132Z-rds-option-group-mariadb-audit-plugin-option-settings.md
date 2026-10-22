Issue-Source: trace-sweep-divergence
Issue-Trace: rds-option-group-add-remove-option
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-option-group-add-remove-option rds-option-group-lifecycle-batch-08 rds-option-group-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*]
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].AllowedValues
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].ApplyType
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].DefaultValue
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].Description
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].Value
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*]
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].AllowedValues
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].ApplyType
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].DefaultValue
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].Description
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].Value
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS option group MARIADB_AUDIT_PLUGIN option settings catalog diverges from AWS

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyOptionGroup

## Description
When an option is added to an RDS option group, the OptionSettings the emulator materialises for that option are a different set from the ones AWS defines: different setting names at each index, different AllowedValues, different Descriptions, a missing setting, and a DefaultValue/Value taken from the wrong setting. The same wrong catalog is echoed by ModifyOptionGroup and by the subsequent DescribeOptionGroups read, so four clusters diverge. All four answer 200; the fault is the option-settings definition table.

Cause 1 of 4: ModifyOptionGroup echoes the wrong OptionSettings for the added option.
AWS: Smithy: ModifyOptionGroup -> OptionGroup.Options: OptionsList of Option {OptionName, OptionSettings: OptionSettingsList of OptionSetting {Name, Value, DefaultValue, Description, ApplyType, DataType, AllowedValues, IsModifiable, IsCollection}}. Recorded behavior only for the per-option setting definitions: AWS's first setting carries AllowedValues 'CONNECT,QUERY,QUERY_DDL,QUERY_DML,QUERY_DCL,QUERY_DML_NO_SELECT' and Description 'Types of actions to be logged (connect, query, or table)', and the option includes a SERVER_AUDIT setting with AllowedValues/DefaultValue/Value 'FORCE_PLUS_PERMANENT', ApplyType STATIC, IsModifiable false.
Emulator: OptionSetting[0].AllowedValues is 'CONNECT,QUERY,QUERY_DDL,QUERY_DML,QUERY_DML_NO_SELECT,QUERY_DCL,TABLE' (different members and order), OptionSetting[0].Description is 'The event classes recorded in the audit log.', OptionSetting[1].AllowedValues is absent where AWS has 'ON', the SERVER_AUDIT setting at index [3] is absent entirely, and the Descriptions at indices [1],[2],[4],[5],[7],[8] plus ApplyType/DefaultValue/Value at [6] diverge.
Why: a client that reads OptionSettings to learn what values a setting accepts (AllowedValues) will send values AWS rejects, and a client that looks for the non-modifiable SERVER_AUDIT setting does not find it at all.
Fix: the option-settings definition catalog the option-group handlers read, and the OptionSetting serializer for the absent AllowedValues member.
Traces: rds-option-group-add-remove-option#task#2.

Cause 2 of 4: DescribeOptionGroups reports the same wrong OptionSettings.
AWS: Smithy: DescribeOptionGroups -> OptionGroupsList of OptionGroup, same Option/OptionSetting shapes; error OptionGroupNotFoundFault (404). Recorded behavior only for the setting definitions, as cause 1.
Emulator: the identical divergences appear on the read path - OptionSetting[0].AllowedValues/Description, OptionSetting[1].AllowedValues missing, OptionSetting[3] (SERVER_AUDIT) missing, and Description/ApplyType/DefaultValue/Value divergences at [2],[4],[5],[6],[7],[8].
Why: the wrong catalog is not confined to the mutation echo - a client that reads the group back after creating it sees the same wrong settings, so nothing in the API surfaces the AWS definitions.
Fix: the same option-settings definition catalog (shared by the describe path).
Traces: rds-option-group-add-remove-option#task#3.

Cause 3 of 4: the OptionSettings are emitted in a different order, so positional values are wrong too.
AWS: recorded behavior only - at this point AWS's OptionSetting[0] has AllowedValues 'ON', OptionSetting[2] is the SERVER_AUDIT setting, and OptionSetting[5] has DefaultValue and Value '/rdsdbdata/log/audit/'.
Emulator: OptionSetting[0].AllowedValues is the event-class list instead of 'ON', OptionSetting[2] is SERVER_AUDIT_INCL_USERS instead of SERVER_AUDIT, OptionSetting[5].DefaultValue and .Value are '9' instead of the audit log path, and OptionSetting[6].AllowedValues is 'ON,OFF' where AWS has the event-class list.
Why: OptionSettings is an ordered wire list; a client reading a setting positionally, or diffing the group against a desired state, reads another setting's value - here a numeric '9' where AWS reports a filesystem path.
Fix: the same catalog, including the order in which settings are emitted for an option.
Traces: rds-option-group-lifecycle-batch-08#task#4.

Cause 4 of 4: the same divergence on the option-group mutation envelope path.
AWS: recorded behavior only - OptionSetting[0] AllowedValues 'ON' with Description 'Determines if logging is turned on', OptionSetting[1] Description 'Include specified users', OptionSetting[2] the SERVER_AUDIT setting, OptionSetting[3] Description 'Limit on the length of the query string in a record.'.
Emulator: OptionSetting[0] carries the event-class AllowedValues and Description 'The event classes recorded in the audit log.', [1] 'The users whose activity is not recorded in the audit log.', [2] is SERVER_AUDIT_INCL_USERS, [3] 'The path of the audit log file.'.
Why: as causes 1 and 3.
Fix: the same catalog.
Traces: rds-option-group-mutation-envelopes#task#5.

## Evidence
All four clusters replay expected_status 200 / actual_status 200. Cause 1: $.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[0].AllowedValues: value_mismatch (expected 'CONNECT,QUERY,QUERY_DDL,QUERY_DML,QUERY_DCL,QUERY_DML_NO_SELECT', got 'CONNECT,QUERY,QUERY_DDL,QUERY_DML,QUERY_DML_NO_SELECT,QUERY_DCL,TABLE'); OptionSetting[1].AllowedValues expected 'ON', got None; OptionSetting[3] missing (expected the SERVER_AUDIT setting with AllowedValues/DefaultValue/Value 'FORCE_PLUS_PERMANENT', ApplyType STATIC, IsModifiable false), got SERVER_AUDIT_FILE_PATH. Cause 2: the same key set under $.DescribeOptionGroupsResponse...OptionSetting[*]. Cause 3: OptionSetting[5].DefaultValue and .Value expected '/rdsdbdata/log/audit/', got '9'; OptionSetting[6].AllowedValues expected the event-class list, got 'ON,OFF'. Cause 4: OptionSetting[0].AllowedValues expected 'ON', got the event-class list.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture rds-option-group-add-remove-option, step task#2 (Action=ModifyOptionGroup adding the audit-plugin option) and diff OptionGroup.Options.Option.OptionSettings.OptionSetting[*] against the recording.
3. Replay task#3 of the same fixture (Action=DescribeOptionGroups) and observe the identical divergences on the read path.
4. Replay rds-option-group-lifecycle-batch-08#task#4 and rds-option-group-mutation-envelopes#task#5 for the ordering and value divergences.
