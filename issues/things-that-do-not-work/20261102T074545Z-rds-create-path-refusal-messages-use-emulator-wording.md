Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle-and-features
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle-and-features rds-global-cluster-mutation-envelopes rds-identifier-validation-and-lowercasing
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: d3219d15fc84d53b82e8eca9b445ec19295fafb3

# RDS create-path refusal messages use emulator wording instead of AWS text

> _Note: this issue was identified against the code base at commit `d3219d15fc84d53b82e8eca9b445ec19295fafb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Four RDS create-path refusals answer the right HTTP status and the right error code but carry the emulator crate's own sentence in $.ErrorResponse.Error.Message instead of the sentence RDS words for that refusal class. In two of them the instance-path and cluster-path identifier templates are swapped: the instance path emits the cluster-style 'Invalid database ... identifier:' line while the cluster path emits a hand-written 'first character ... must be a letter' line, which is the mirror image of AWS. 4 causes.

Cause 1 of 4: CreateDBInstance naming a DB subnet group that does not exist.
AWS: Smithy: CreateDBInstance declares DBSubnetGroupNotFoundFault, errorKind client, httpError 404, awsQueryError code DBSubnetGroupNotFoundFault; the message text itself is recorded behavior only. The recorded 404 Message is "DBSubnetGroup '<DBSubnetGroupName>' not found." Fixed text: "DBSubnetGroup '" ... "' not found." — the type name is the modeled shape name DBSubnetGroup, one word, no internal space. The only variable part is the request's DBSubnetGroupName, single-quoted.
Emulator: 404 with the same error code but Message "DB Subnet group '<name>' not found." — the type name is re-spelled as three space-separated, differently-cased words.
Why: a client that branches on RDS's missing-subnet-group sentence (or asserts it in a test) never matches, so a create against a non-existent subnet group is indistinguishable from an unrelated 404.
Fix: the CreateDBInstance handler's DB subnet group existence check — the message it hands to the DBSubnetGroupNotFoundFault it raises.
Traces: rds-db-instance-core-lifecycle-and-features#task#1.
Note: the open issue 20261029T214632Z-rds-not-found-refusals-for-instances-clusters-subnet.md describes the same sentence but does not carry this step's replay key (its Issue-Key list covers DescribeValidDBInstanceModifications, CreateTenantDatabase, ModifyDBCluster, RevokeDBSecurityGroupIngress and DescribeDBClusterAutomatedBackups, not CreateDBInstance), so this cause is still unowned and is filed here.

Cause 2 of 4: CreateGlobalCluster with an Engine that cannot back a global database.
AWS: Smithy: CreateGlobalCluster.Engine is a plain String with no enum or @pattern, so the admissible engine set is a service-side rule, not a modeled one; the refusal text is recorded behavior only. The recorded refusal is HTTP 400 with Message 'The requested engine version was not found or does not support global functionality' — entirely fixed text, with no request value interpolated and no terminal period.
Emulator: 400 with Message 'invalid value mysql for Engine: a global database cluster runs one of aurora-mysql, aurora-postgresql' — the crate's own diagnostic, which both re-words the refusal and leaks the rejected request value and an engine list AWS does not print.
Why: AWS deliberately keeps this sentence value-free; a caller matching it sees no match, and a caller parsing the emulator's list builds logic against text AWS never emits.
Fix: the CreateGlobalCluster handler's engine/engine-version validation message.
Traces: rds-global-cluster-mutation-envelopes#task#4.

Cause 3 of 4: CreateDBInstance with a DBInstanceIdentifier that violates the identifier grammar (trailing hyphen / consecutive hyphens).
AWS: Smithy: CreateDBInstance.DBInstanceIdentifier targets plain String with no @pattern, so the identifier grammar is enforced by the service and its refusal text is recorded behavior only. The recorded refusal is HTTP 400 with Message 'The parameter DBInstanceIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' Fixed text: the whole sentence pair, stating the rule; the only variable part is the member name DBInstanceIdentifier, which is the input member that failed — the rejected value is NOT echoed.
Emulator: 400 with Message 'Invalid database identifier:  <rejected value>' — a different template that drops AWS's statement of the rule and instead echoes the rejected identifier.
Why: callers get neither the rule AWS documents in the message nor a matchable sentence; and the value echo exposes request text where AWS prints none.
Fix: the CreateDBInstance handler's identifier validation (the InvalidParameterValue it raises for DBInstanceIdentifier).
Traces: rds-identifier-validation-and-lowercasing#task#2; also rds-identifier-validation-and-lowercasing#task#3.

Cause 4 of 4: CreateDBCluster with a DBClusterIdentifier whose first character is not a letter.
AWS: Smithy: CreateDBCluster.DBClusterIdentifier targets plain String with no @pattern (unlike the sibling GlobalClusterIdentifier, which is @pattern '^[A-Za-z][0-9A-Za-z-:._]*$'), so this refusal is recorded behavior only. The recorded refusal is HTTP 400 with Message 'Invalid database cluster identifier:  <DBClusterIdentifier>' — fixed text 'Invalid database cluster identifier:' followed by two spaces, then the rejected identifier taken verbatim from the request; the double space is part of AWS's fixed text.
Emulator: 400 with Message "the first character of DBClusterIdentifier must be a letter; got '<value>'" — a lower-cased, hand-rolled diagnostic in place of AWS's template. Together with cause 3 this is a swap: the AWS cluster template appears on the instance path and a hand-written rule sentence appears on the cluster path.
Why: neither path matches AWS, so identifier-validation handling written against RDS fails on both the instance and the cluster create.
Fix: the CreateDBCluster handler's identifier validation message (and the shared identifier-validation helper the instance path uses, so the two templates stop being crossed).
Traces: rds-identifier-validation-and-lowercasing#task#5; also rds-identifier-validation-and-lowercasing#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. In all four clusters the HTTP status and the error code matched; the only diverging key was $.ErrorResponse.Error.Message.

1. rds-db-instance-core-lifecycle-and-features#task#1 CreateDBInstance, status 404 expected and 404 actual:
   expected "DBSubnetGroup 'sim2real-probe-rds-subnetgrp-missing-c6a7d4cb' not found."
   actual   "DB Subnet group 'sim2real-probe-rds-subnetgrp-missing-c6a7d4cb' not found."
   (the quoted name is the request's DBSubnetGroupName — a per-run value, not a requirement; the requirement is the one-word type name DBSubnetGroup and the fixed "' not found." tail.)

2. rds-global-cluster-mutation-envelopes#task#4 CreateGlobalCluster, status 400 expected and 400 actual:
   expected 'The requested engine version was not found or does not support global functionality'
   actual   'invalid value mysql for Engine: a global database cluster runs one of aurora-mysql, aurora-postgresql'
   (the expected sentence is entirely fixed text; the actual one interpolates the request's Engine.)

3. rds-identifier-validation-and-lowercasing#task#2 CreateDBInstance, status 400 expected and 400 actual (cluster also covers #task#3):
   expected 'The parameter DBInstanceIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.'
   actual   'Invalid database identifier:  rdserr-db-59a9a1f4-'
   (fixed text is the whole expected sentence pair with the member name DBInstanceIdentifier; 'rdserr-db-59a9a1f4-' is the run's rejected value, which AWS does not print at all.)

4. rds-identifier-validation-and-lowercasing#task#5 CreateDBCluster, status 400 expected and 400 actual (cluster also covers #task#6):
   expected 'Invalid database cluster identifier:  1rdserr-cl-59a9a1f4'
   actual   "the first character of DBClusterIdentifier must be a letter; got '1rdserr-cl-59a9a1f4'"
   (fixed text is 'Invalid database cluster identifier:' plus the two spaces; '1rdserr-cl-59a9a1f4' is the request's DBClusterIdentifier.)

Smithy evidence used (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): CreateDBInstance errors include DBSubnetGroupNotFoundFault (client, httpError 404, awsQueryError DBSubnetGroupNotFoundFault); CreateDBInstance.DBInstanceIdentifier and CreateDBCluster.DBClusterIdentifier are plain String with no @pattern; CreateGlobalCluster.Engine is plain String with no enum, while CreateGlobalCluster.GlobalClusterIdentifier is @length(1..255) @pattern '^[A-Za-z][0-9A-Za-z-:._]*$'.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire corpus for trace set simparity-wire-aws-v1.
3. Re-run the representative step rds-db-instance-core-lifecycle-and-features#task#1 (CreateDBInstance naming a DBSubnetGroupName that was never created) and compare $.ErrorResponse.Error.Message against the recording.
4. Re-run the steps named in trace_ids, one per remaining cause:
   - rds-global-cluster-mutation-envelopes#task#4 — CreateGlobalCluster with Engine=mysql.
   - rds-identifier-validation-and-lowercasing#task#2 — CreateDBInstance with a DBInstanceIdentifier ending in a hyphen (its cluster also covers task#3).
   - rds-identifier-validation-and-lowercasing#task#5 — CreateDBCluster with a DBClusterIdentifier starting with a digit (its cluster also covers task#6).
5. In each, the status and error code match the recording and only Message diverges; the fix is complete when all four messages match AWS's wording with only the request-derived values substituted.
