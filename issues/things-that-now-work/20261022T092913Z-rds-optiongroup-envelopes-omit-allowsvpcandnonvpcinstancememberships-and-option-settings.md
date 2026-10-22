Issue-Source: trace-sweep-divergence
Issue-Trace: rds-already-exists-conflict-class
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-already-exists-conflict-class rds-option-group-mutation-envelopes rds-option-group-add-remove-option rds-option-group-lifecycle-batch-08
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateOptionGroup|2xx->2xx|$.CreateOptionGroupResponse.CreateOptionGroupResult.OptionGroup.AllowsVpcAndNonVpcInstanceMemberships
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.AllowsVpcAndNonVpcInstanceMemberships
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS OptionGroup envelopes omit AllowsVpcAndNonVpcInstanceMemberships and option settings

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOptionGroup

## Description
Three causes in the OptionGroup projection.

Cause 1 of 3 — CreateOptionGroup response. AWS: OptionGroup carries AllowsVpcAndNonVpcInstanceMemberships (Smithy: com.amazonaws.rds#OptionGroup member AllowsVpcAndNonVpcInstanceMemberships, Boolean — 'Indicates whether this option group can be applied to both VPC and non-VPC instances'), and the recording shows 'true' for a freshly created group. Emulator: the member is absent from CreateOptionGroupResult.OptionGroup. Why: a client reading AllowsVpcAndNonVpcInstanceMemberships (and the related VpcId semantics documented on that member) gets None and cannot tell whether the group is VPC-restricted. Fix: the OptionGroup serializer / state default. Traces: rds-already-exists-conflict-class#task#1.

Cause 2 of 3 — DescribeOptionGroups. Same member absent from every OptionGroupsList.OptionGroup entry. Fix: the same OptionGroup projection, on the describe path. Traces: rds-already-exists-conflict-class#task#7.

Cause 3 of 3 — DescribeOptionGroups after ModifyOptionGroup added an option. AWS: the group's Options list carries an Option element with OptionName, OptionDescription, Permanent, Persistent, VpcSecurityGroupMemberships, DBSecurityGroupMemberships and the option's OptionSettings (each with Name, DataType, ApplyType, DefaultValue, AllowedValues, Value, IsCollection, IsModifiable, Description) — recorded for MARIADB_AUDIT_PLUGIN. Emulator: Options is the wrong type/shape (the option-settings catalog is not served) and AllowsVpcAndNonVpcInstanceMemberships is again missing (expected 'false' here). Why: clients that read an added option's settings/defaults see nothing, so option-group configuration flows cannot be validated. Fix: the option catalog backing ModifyOptionGroup plus the OptionGroup.Options serializer. Traces: rds-option-group-add-remove-option#task#3.

## Evidence
Cause 1: rds-already-exists-conflict-class#task#1 (also rds-option-group-mutation-envelopes#task#1) — $.CreateOptionGroupResponse.CreateOptionGroupResult.OptionGroup.AllowsVpcAndNonVpcInstanceMemberships missing (expected 'true', got None).
Cause 2: rds-already-exists-conflict-class#task#7 (also rds-option-group-add-remove-option#task#5, rds-option-group-lifecycle-batch-08#task#3) — $.DescribeOptionGroupsResponse...OptionGroup.AllowsVpcAndNonVpcInstanceMemberships missing (expected 'true', got None).
Cause 3: rds-option-group-add-remove-option#task#3 — AllowsVpcAndNonVpcInstanceMemberships missing (expected 'false'); $...OptionGroup.Options type_mismatch (expected an Option with OptionName 'MARIADB_AUDIT_PLUGIN' and its OptionSettings list).

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) Fixture rds-already-exists-conflict-class task#1: CreateOptionGroup and read CreateOptionGroupResult/OptionGroup — no AllowsVpcAndNonVpcInstanceMemberships element.
3) Same fixture task#7: DescribeOptionGroups — same member absent.
4) Fixture rds-option-group-add-remove-option task#3: ModifyOptionGroup to add MARIADB_AUDIT_PLUGIN, then DescribeOptionGroups and inspect OptionGroup/Options/Option/OptionSettings.
