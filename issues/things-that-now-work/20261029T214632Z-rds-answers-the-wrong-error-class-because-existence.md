Issue-Source: trace-sweep-divergence
Issue-Trace: rds-custom-engine-version
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-custom-engine-version rds-create-db-cluster-nonexistent-cluster-parameter-group rds-event-subscription-lifecycle rds-parameter-group-enforcement rds-blue-green-deployment-errors rds-event-subscription-mutation-envelopes rds-identifier-validation-and-lowercasing
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyCustomDBEngineVersion|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyCustomDBEngineVersion|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyCustomDBEngineVersion|4xx->4xx|status
Issue-Key: rds|CreateCustomDBEngineVersion|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->5xx|$.ErrorResponse.Error.Code
Issue-Key: rds|CreateDBCluster|4xx->5xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->5xx|$.ErrorResponse.Error.Type
Issue-Key: rds|CreateDBCluster|4xx->5xx|status
Issue-Key: rds|ModifyEventSubscription|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyEventSubscription|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyEventSubscription|4xx->4xx|status
Issue-Key: rds|ModifyDBParameterGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: rds|ModifyDBParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|ModifyDBParameterGroup|4xx->4xx|status
Issue-Key: rds|CreateBlueGreenDeployment|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateEventSubscription|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS answers the wrong error class because existence, permission and validation checks run out of order

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCustomDBEngineVersion

## Description
Eight refusals pick a different error class (and in four cases a different HTTP status) from the one AWS raises for the same request, because the emulator evaluates existence/permission/validation guards in a different order or words the chosen class itself. 8 causes.

Cause 1 of 8: ModifyCustomDBEngineVersion on a CEV that does not exist.
AWS: recorded 403, Code AccessDenied, Type Sender, Message "The custom engine version you requested may not exist or you don't have the required permission." — entirely fixed text; AWS deliberately does not confirm existence.
Emulator: 404 with Code CustomDBEngineVersionNotFoundFault and Message 'Custom engine version <EngineVersion> not found for engine <Engine>' — confirms non-existence and names request values.
Why: the client gets the wrong status class and the emulator discloses existence AWS hides.
Fix: the ModifyCustomDBEngineVersion handler's authorization/existence ordering.
Traces: rds-custom-engine-version#task#1.

Cause 2 of 8: CreateCustomDBEngineVersion for an engine that is not a custom engine.
AWS: recorded 400, Message 'Invalid DB engine' — entirely fixed text; the engine check runs first.
Emulator: 400 with 'KMSKeyId must be specified for engine <Engine>: a symmetric encryption KMS key is required for an RDS Custom engine version' — the KMS-key guard runs before the engine check, so a different cause is reported.
Why: the client is told to supply a KMS key for a request that can never succeed.
Fix: the CreateCustomDBEngineVersion handler's validation ordering.
Traces: rds-custom-engine-version#task#3.

Cause 3 of 8: CreateDBCluster naming a DB cluster parameter group that does not exist, with ManageMasterUserPassword set.
AWS: recorded 404, Type Sender, Code DBClusterParameterGroupNotFound, Message 'DBClusterParameterGroup not found: <DBClusterParameterGroupName>'.
Emulator: 501 Type Receiver, Code NotImplemented, with the emulator's ManageMasterUserPassword stub message — the unimplemented-feature guard runs before the parameter-group existence check.
Why: a client probing a missing parameter group gets a server-fault class and never learns the group is absent; the 501 masks a modeled client error.
Fix: the CreateDBCluster handler — run referenced-resource existence checks before unsupported-feature refusals.
Traces: rds-create-db-cluster-nonexistent-cluster-parameter-group#task#3. (Open issue 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md covers the ManageMasterUserPassword 501 itself; it does not cover this ordering, and this cluster carried no owner.)

Cause 4 of 8: ModifyEventSubscription with EventCategories and a null SourceType on a subscription that does not exist.
AWS: recorded 400, Code InvalidParameterCombination, Message 'If SourceType is null, Categories must also be null.' — the parameter-combination check runs before existence.
Emulator: 404, Code SubscriptionNotFound, Message 'Event Subscription <SubscriptionName> not found.'
Why: wrong status and class for a malformed request.
Fix: the ModifyEventSubscription handler's validation/existence ordering.
Traces: rds-event-subscription-lifecycle#task#8.

Cause 5 of 8: ModifyDBParameterGroup targeting a default parameter group.
AWS: recorded 400, Code InvalidParameterValue, Message 'Cannot modify a default parameter group.' — AWS knows the default group exists and refuses the modification.
Emulator: 404, Code DBParameterGroupNotFound, Message 'DBParameterGroup not found: default.mysql8.0' — the emulator has no default parameter groups at all.
Why: the client cannot distinguish "protected" from "absent", and the engine's default groups are missing from state.
Fix: the ModifyDBParameterGroup handler plus the default-parameter-group seed in RDS state.
Traces: rds-parameter-group-enforcement#task#2.

Cause 6 of 8: CreateBlueGreenDeployment with a Source ARN in another Region.
AWS: recorded 400, Message "'<Source>' is not a valid DB Instance ARN in this region". Fixed text: "'" + "' is not a valid DB Instance ARN in this region"; the variable part is the Source ARN from the request, single-quoted.
Emulator: 400 with "invalid value '<Source>' for Source: the source database must be in <region>, the region this request is signed for" — the emulator's own prose, naming the signing Region.
Why: the message AWS guarantees is not reproduced.
Fix: the CreateBlueGreenDeployment handler's Source validation message.
Traces: rds-blue-green-deployment-errors#task#2.

Cause 7 of 8: CreateEventSubscription with an unknown SourceType.
AWS: recorded 400, Message "Invalid event source type. Valid types are 'db-instance', 'db-security-group', 'db-parameter-group', 'db-snapshot', 'db-cluster', 'db-cluster-snapshot', 'custom-engine-version', 'db-proxy', 'blue-green-deployment', 'db-shard-group', 'zero-etl'" — entirely fixed text, including the order and quoting of the valid list.
Emulator: 'invalid value <SourceType> for SourceType: must be one of db-instance, db-cluster, db-parameter-group, db-security-group, db-snapshot, db-cluster-snapshot, db-proxy, zero-etl, custom-engine-version, blue-green-deployment' — different sentence, unquoted list, different order, and the list omits db-shard-group.
Why: the client is given a wrong set of valid source types.
Fix: the CreateEventSubscription handler's SourceType validation message and value list.
Traces: rds-event-subscription-mutation-envelopes#task#6.

Cause 8 of 8: CreateDBParameterGroup with an invalid DBParameterGroupName.
AWS: recorded 400, Message 'The parameter DBParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' Fixed text as quoted, with the member name as its only variable part; the offending value is NOT echoed.
Emulator: "Invalid value '<name>' for DBParameterGroupName: it must contain from 1 to 255 letters, numbers, or hyphens, the first character must be a letter, and it cannot end with a hyphen or contain two consecutive hyphens" — echoes the value and states the rule in its own words.
Why: same message-matching break, on a rule AWS words identically for every identifier member.
Fix: the CreateDBParameterGroup handler's identifier validation message (shared identifier-rule template).
Traces: rds-identifier-validation-and-lowercasing#task#11.

## Evidence
Replay cluster details:
1. rds-custom-engine-version#task#1 ModifyCustomDBEngineVersion: status expected 403 got 404; Code expected 'AccessDenied' got 'CustomDBEngineVersionNotFoundFault'; Message expected "The custom engine version you requested may not exist or you don't have the required permission." got 'Custom engine version sim2real-nonexistent-cev not found for engine custom-oracle-ee'.
2. rds-custom-engine-version#task#3 CreateCustomDBEngineVersion 400->400: Message expected 'Invalid DB engine', got 'KMSKeyId must be specified for engine custom-oracle-ee: ...'.
3. rds-create-db-cluster-nonexistent-cluster-parameter-group#task#3 CreateDBCluster: status expected 404 got 501; Type expected 'Sender' got 'Receiver'; Code expected 'DBClusterParameterGroupNotFound' got 'NotImplemented'; Message expected 'DBClusterParameterGroup not found: sim2real-probe-rds-absent-cpg-ec6bc72a'.
4. rds-event-subscription-lifecycle#task#8 ModifyEventSubscription: status expected 400 got 404; Code expected 'InvalidParameterCombination' got 'SubscriptionNotFound'.
5. rds-parameter-group-enforcement#task#2 ModifyDBParameterGroup: status expected 400 got 404; Code expected 'InvalidParameterValue' got 'DBParameterGroupNotFound'; Message expected 'Cannot modify a default parameter group.' got 'DBParameterGroup not found: default.mysql8.0'.
6. rds-blue-green-deployment-errors#task#2 CreateBlueGreenDeployment 400->400: Message expected "'arn:aws:rds:us-east-1:000000000000:db:nonexistent-db-probe' is not a valid DB Instance ARN in this region", got "invalid value '...' for Source: the source database must be in us-west-2, the region this request is signed for".
7. rds-event-subscription-mutation-envelopes#task#6 CreateEventSubscription 400->400: Message expected the quoted eleven-value valid-types sentence, got the emulator's ten-value prose.
8. rds-identifier-validation-and-lowercasing#task#11 CreateDBParameterGroup 400->400: Message expected 'The parameter DBParameterGroupName is not a valid identifier. ...', got "Invalid value 'rdserr-pg-59a9a1f4-' for DBParameterGroupName: ...".

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-custom-engine-version#task#1 and every step in trace_ids; each sends one request that trips two guards (or a single mis-worded guard) and the replay compares status, $.ErrorResponse.Error.Code and .Message.
