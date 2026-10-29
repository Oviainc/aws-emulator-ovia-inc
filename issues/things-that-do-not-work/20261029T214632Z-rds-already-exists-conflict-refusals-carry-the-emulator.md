Issue-Source: trace-sweep-divergence
Issue-Trace: rds-already-exists-conflict-class
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-already-exists-conflict-class rds-db-security-group-mutation-envelopes rds-global-cluster-mutation-envelopes rds-create-db-instance-catalog-triple rds-identifier-validation-and-lowercasing rds-db-cluster-lifecycle rds-db-cluster-lifecycle-with-snapshot rds-duplicate-create-error rds-delete-groups-in-use-by-instance
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBSecurityGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeleteOptionGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS already-exists conflict refusals carry the emulator's own message template instead of AWS's

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSecurityGroup

## Description
Five conflict/in-use refusals answer the right HTTP status and error code but word the Message with the emulator's own "<Thing> already exists: <id>" template rather than AWS's sentence for that error class. Clients and SDK-level assertions that match on the message text see a different string. 5 causes.

Cause 1 of 5: CreateDBSecurityGroup on a name that already exists.
AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19) CreateDBSecurityGroup errors DBSecurityGroupAlreadyExistsFault, awsQueryError code DBSecurityGroupAlreadyExists, httpError 400. Recorded body: 400, Code DBSecurityGroupAlreadyExists, Message 'A security group named <DBSecurityGroupName> already exists'. Fixed text: "A security group named " + " already exists"; the only variable part is the DBSecurityGroupName taken from the request.
Emulator: 400 with Message 'DB Security Group already exists: <DBSecurityGroupName>' — a different sentence, with the identifier appended after a colon instead of embedded.
Why: a client that branches on the message (or a recorded-response assertion) does not recognise the duplicate-name case.
Fix: the CreateDBSecurityGroup handler's conflict path / the RDS error-message templates.
Traces: rds-already-exists-conflict-class#task#4; also rds-db-security-group-mutation-envelopes#task#2, #task#3.

Cause 2 of 5: CreateGlobalCluster on an existing GlobalClusterIdentifier.
AWS: Smithy CreateGlobalCluster errors GlobalClusterAlreadyExistsFault, httpError 400. Recorded: 400, Message 'Global cluster <GlobalClusterIdentifier> already exists'. Fixed text: "Global cluster " + " already exists"; variable part is the request's GlobalClusterIdentifier.
Emulator: 400 with 'Global cluster already exists: <GlobalClusterIdentifier>'.
Why: same message-matching break on the global-cluster conflict class.
Fix: the CreateGlobalCluster handler's conflict path.
Traces: rds-already-exists-conflict-class#task#6; also rds-global-cluster-mutation-envelopes#task#3, #task#4.

Cause 3 of 5: CreateDBInstance on an existing DBInstanceIdentifier.
AWS: recorded behavior only — 400, Message is the fixed sentence 'DB instance already exists' with NO identifier interpolated.
Emulator: 400 with 'DB instance already exists: <DBInstanceIdentifier>' — appends the request identifier AWS does not include.
Why: the emulator leaks a request value into a message AWS keeps constant, so exact-match assertions fail.
Fix: the CreateDBInstance handler's conflict path.
Traces: rds-create-db-instance-catalog-triple#task#13; also rds-identifier-validation-and-lowercasing#task#2, #task#3.

Cause 4 of 5: CreateDBCluster on an existing DBClusterIdentifier.
AWS: recorded behavior only — 400, Message is the fixed sentence 'DB Cluster already exists', no identifier.
Emulator: 400 with 'DB Cluster already exists: <DBClusterIdentifier>'.
Why: same — an identifier is appended to a message AWS words without one.
Fix: the CreateDBCluster handler's conflict path.
Traces: rds-db-cluster-lifecycle#task#2; also rds-db-cluster-lifecycle-with-snapshot#task#6, rds-duplicate-create-error#task#2, rds-identifier-validation-and-lowercasing#task#5, #task#6.

Cause 5 of 5: DeleteOptionGroup on an option group attached to a DB instance.
AWS: Smithy DeleteOptionGroup errors InvalidOptionGroupStateFault, httpError 400. Recorded: 400, Message "The option group '<OptionGroupName>' cannot be deleted because it is in use." Fixed text: "The option group '" + "' cannot be deleted because it is in use."; variable part is the request's OptionGroupName, single-quoted.
Emulator: 400 with "The option group <OptionGroupName> can't be deleted because it is in use by DB instance <DBInstanceIdentifier>" — drops the quotes, contracts "cannot", drops the terminal period and adds a holder identifier AWS does not name.
Why: the sentence AWS guarantees is not reproduced, and the emulator discloses a value AWS withholds.
Fix: the DeleteOptionGroup handler's in-use precondition message.
Traces: rds-delete-groups-in-use-by-instance#task#9.

Related but not covering: open issue 20261029T195009Z-rds-8-net-new-root-causes-not-found.md concerns other mis-worded refusals; none of the five clusters above is bound to it (owned_by was empty for all five).

## Evidence
Replay (trace set simparity-wire-aws-v1) cluster details, expected vs actual Message:
1. rds-already-exists-conflict-class#task#4 CreateDBSecurityGroup 400->400: $.ErrorResponse.Error.Message expected 'A security group named rdserr-dup-sg-4adb38ab already exists', got 'DB Security Group already exists: rdserr-dup-sg-4adb38ab'.
2. rds-already-exists-conflict-class#task#6 CreateGlobalCluster 400->400: expected 'Global cluster rdserr-dup-gc-4adb38ab already exists', got 'Global cluster already exists: rdserr-dup-gc-4adb38ab'.
3. rds-create-db-instance-catalog-triple#task#13 CreateDBInstance 400->400: expected 'DB instance already exists', got 'DB instance already exists: rdsenv-db-59d2cdc9'.
4. rds-db-cluster-lifecycle#task#2 CreateDBCluster 400->400: expected 'DB Cluster already exists', got 'DB Cluster already exists: sim2real-probe-rds-sim2real-probe-cluster-4ba5a001'.
5. rds-delete-groups-in-use-by-instance#task#9 DeleteOptionGroup 400->400: expected "The option group 'rdserr-iu-og-b1da8497' cannot be deleted because it is in use.", got "The option group rdserr-iu-og-b1da8497 can't be deleted because it is in use by DB instance rdserr-iu-db-b1da8497".
(The quoted identifiers are this run's generated names; the requirement is the fixed sentence plus the request's own identifier.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Inspect the representative step rds-already-exists-conflict-class#task#4 and the steps listed in trace_ids; each creates the resource once, then repeats the create (or deletes an in-use option group) and compares $.ErrorResponse.Error.Message against the recording.
