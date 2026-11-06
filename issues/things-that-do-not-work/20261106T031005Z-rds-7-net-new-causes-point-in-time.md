Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.e2e-point-in-time-restore-recovers-data rds.point-in-time-restore-data-query-round-trip rds.static-parameter-pending-reboot-query-effect rds.log-file-portion-contains-the-statement-the-engine-executed rds.cloudwatch-logs-export-delivers-only-while-enabled rds.modify-instance-weak-master-password-rejected rds.deferred-class-modification-stays-pending-while-instance-serves-sql rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.e2e-point-in-time-restore-recovers-data|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: rds.point-in-time-restore-data-query-round-trip|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: rds.static-parameter-pending-reboot-query-effect|_poll#25|Timeout|time.sleep(interval_s)
Issue-Key: rds.log-file-portion-contains-the-statement-the-engine-executed|run#30|TimeoutError|raise TimeoutError(f"{marker!r} in the RDS log: the run budget ended first; not graded")
Issue-Key: rds.cloudwatch-logs-export-delivers-only-while-enabled|_poll_until#27|AssertionError|raise AssertionError(
Issue-Key: rds.modify-instance-weak-master-password-rejected|run#17|AssertionError|assert settled["DBInstanceStatus"] == "available", (
Issue-Key: rds.deferred-class-modification-stays-pending-while-instance-serves-sql|run#12|AssertionError|assert resp["DBInstanceClass"] == "db.t3.micro", (
Issue-Key: rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows|_wait_stack#17|AssertionError|raise AssertionError(f"stack {stack_name} reached {st!r} waiting for {want!r}: {reasons!r}")
Issue-Key: rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance|_wait_stack#17|AssertionError|raise AssertionError(f"stack {stack_name} reached {st!r} waiting for {want!r}: {reasons!r}")
Issue-Commit: 1ea8394ddf89938ba9051f7092160f10eda1506a

# rds: 7 net-new causes — point-in-time restore and static-parameter reboot never settle, DB log files never contain the executed statement, CloudwatchLogsExportConfiguration is never applied, ModifyDBInstance leaves the instance 'modifying', ApplyImmediately=false reports the new class, and CloudFormation secretsmanager dynamic references are not resolved

> _Note: this issue was identified against the code base at commit `1ea8394ddf89938ba9051f7092160f10eda1506a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBInstance

## Description
This issue bundles 7 net-new root causes in the rds service, all grounded in failing behavioural checks the suite report marked neither ALREADY FILED nor OWNED. Cause 6 is adjacent to the open issue 20261104T053605Z-rds-6-net-new-causes-applyimmediately-false-is.md, but that issue does not carry this check's replay key, so the deferred-class projection is filed here. A fixer must close all SEVEN causes below before submitting.

=== Cause 1 of 7: point-in-time restore never completes, so a restored instance is never reachable ===
AWS: recorded behavior only for the timings; the contract is RestoreDBInstanceToPointInTime — a restore started against a source instance with automated backups enabled creates a new DB instance that progresses creating -> available and whose data is the source's state at the requested RestoreTime (rows committed before it present, rows committed after it absent).
Emulator: the flow never reaches a verdict — the check is cut off by the harness cap with no terminal state observed, so no restored instance ever becomes available and the restored data can never be read.
Why: a client's whole backup/recovery story is unusable: RestoreDBInstanceToPointInTime is the documented recovery path, and a restore that never settles is indistinguishable from a hung control plane.
Fix: the RestoreDBInstanceToPointInTime handler and the restored-instance lifecycle state machine (creating -> available) plus the data-plane snapshot the restore is materialised from.
Traces: rds.e2e-point-in-time-restore-recovers-data, rds.point-in-time-restore-data-query-round-trip.

=== Cause 2 of 7: a static DB-parameter change plus RebootDBInstance never produces an observable effect ===
AWS: recorded behavior only. A static parameter modified with ApplyMethod=pending-reboot shows ParameterApplyStatus pending-reboot, the instance reports a pending-reboot maintenance state, and after RebootDBInstance the instance returns to available with the new value readable from a session.
Emulator: the flow never reaches a verdict — it is cut off by the harness cap with the parameter's effect never observable.
Why: a client cannot tell whether a parameter-group change took effect, and the pending-reboot/apply cycle that RDS documents as the only way to change a static parameter cannot be driven to completion.
Fix: the ModifyDBParameterGroup/RebootDBInstance handlers' interaction with the instance state machine, and the engine data plane's reading of applied parameter values.
Traces: rds.static-parameter-pending-reboot-query-effect.

=== Cause 3 of 7: DB log files never contain the statements the engine executed ===
AWS: recorded behavior only; the contract is DescribeDBLogFiles / DownloadDBLogFilePortion — a statement executed on the instance (with the engine's statement logging on) appears in the instance's log file, so DownloadDBLogFilePortion returns LogFileData containing it.
Emulator: a uniquely marked statement executed through the engine never appears in any log-file portion within the run budget; the check ends TimeoutError: '<marker>' in the RDS log: the run budget ended first.
Why: log-based auditing, slow-query analysis and troubleshooting are impossible — the log files exist but are not fed by the data plane, so an operator sees an empty or synthetic log for an instance that is serving SQL.
Fix: wire the engine data plane's statement execution into the log-file store that DescribeDBLogFiles / DownloadDBLogFilePortion read.
Traces: rds.log-file-portion-contains-the-statement-the-engine-executed.

=== Cause 4 of 7: ModifyDBInstance's CloudwatchLogsExportConfiguration is never reflected in EnabledCloudwatchLogsExports ===
AWS: Smithy: ModifyDBInstanceMessage.CloudwatchLogsExportConfiguration (CloudwatchLogsExportConfiguration.EnableLogTypes/DisableLogTypes); the enabled types appear in PendingModifiedValues.PendingCloudwatchLogsExports while pending and in DBInstance.EnabledCloudwatchLogsExports once applied, and are removed again when disabled.
Emulator: after enabling the postgresql log type the instance's EnabledCloudwatchLogsExports stays [] for the whole observation window — the modification is accepted but never applied or projected.
Why: a client cannot turn log export on or off, and cannot read back which log types are exporting; the enable/disable toggle silently does nothing while the API reports success.
Fix: the ModifyDBInstance handler's CloudwatchLogsExportConfiguration application into instance state, and the EnabledCloudwatchLogsExports / PendingCloudwatchLogsExports projection in the DBInstance serializer.
Traces: rds.cloudwatch-logs-export-delivers-only-while-enabled.

=== Cause 5 of 7: ModifyDBInstance leaves the instance stuck in 'modifying' after the change has been applied ===
AWS: recorded behavior only for the timing; the contract is that a DB instance leaves DBInstanceStatus=modifying and returns to available once the modification (here an accepted MasterUserPassword change) has been applied, and PendingModifiedValues no longer carries it.
Emulator: once the accepted password change has cleared as applied, DescribeDBInstances still reports DBInstanceStatus 'modifying' and never returns to 'available'.
Why: every client and waiter that gates on DBInstanceStatus=available after a modify blocks forever, and subsequent operations that require an available instance are refused on state.
Fix: the ModifyDBInstance handler's instance state machine — transition modifying -> available when the pending modification is applied.
Traces: rds.modify-instance-weak-master-password-rejected.

=== Cause 6 of 7: ModifyDBInstance with ApplyImmediately=false immediately reports the NEW DBInstanceClass instead of keeping the current one and recording the change as pending ===
AWS: Smithy: ModifyDBInstanceMessage.ApplyImmediately — "If this parameter is disabled, changes to the DB instance are applied during the next maintenance window"; PendingModifiedValues.DBInstanceClass — "The name of the compute and memory capacity class for the DB instance", and PendingModifiedValues "contains changes that will be applied during the next maintenance window". So a deferred class change leaves DBInstance.DBInstanceClass at the CURRENT class and surfaces the requested class only in PendingModifiedValues.DBInstanceClass.
Emulator: the ModifyDBInstance response (and the subsequent read) reports DBInstanceClass as the requested new class straight away, so a deferred modification is indistinguishable from an applied one.
Why: a client cannot tell what the instance is actually running on. Capacity/cost reporting and any logic that compares the live class against the pending one read the deferred value as live, and the maintenance-window application that ApplyImmediately=false exists to express is invisible.
Fix: the ModifyDBInstance handler's deferred-change path and the DBInstance/PendingModifiedValues projection — keep the live class and put the requested class in PendingModifiedValues.DBInstanceClass until it is applied.
Traces: rds.deferred-class-modification-stays-pending-while-instance-serves-sql.

=== Cause 7 of 7: CloudFormation does not resolve the {{resolve:secretsmanager:...}} dynamic reference, so the literal reference string is passed to RDS as MasterUserPassword ===
AWS: recorded behavior only for CloudFormation's dynamic-reference substitution; the RDS side is modelled — CreateDBInstance.MasterUserPassword for MySQL has a maximum length of 41 characters. A template whose AWS::RDS::DBInstance sets MasterUsername/MasterUserPassword to {{resolve:secretsmanager:${AdminSecret}:SecretString:username}} / :password resolves those to the generated secret's values (here a 24-character generated password) BEFORE the resource provider calls CreateDBInstance, and the stack reaches CREATE_COMPLETE.
Emulator: the reference is not resolved; the literal string {{resolve:secretsmanager:<logical-id>:SecretString:password}} is handed to CreateDBInstance, whose own validator then refuses it on length. The DBInstance resource goes CREATE_FAILED with ResourceStatusReason `Resource handler returned message: "The parameter MasterUserPassword is not a valid password because it is longer than 41 characters." (RequestToken: <per-call token>, HandlerErrorCode: InvalidRequest)` and the stack rolls back to ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE. The fixed text of that message is `The parameter MasterUserPassword is not a valid password because it is longer than 41 characters.`; the member name is from the request and the RequestToken is a per-call value. The refusal itself is the SYMPTOM — AWS never reaches it because the value it validates is the resolved 24-character password.
Why: the single most common CloudFormation pattern for RDS credentials — generate a secret and reference it from the DB instance — cannot be deployed at all. Any template using a secretsmanager dynamic reference fails and rolls back, and the failure reason blames the password rather than the unresolved reference, which misdirects the operator.
Fix: the CloudFormation template-processing stage that expands dynamic references ({{resolve:secretsmanager:<secret>:SecretString:<json-key>}}, including the ssm/ssm-secure forms) before resource-provider property values are passed to the service handler.
Traces: rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows, rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance.

## Evidence
Cause 1: rds.e2e-point-in-time-restore-recovers-data — "Check timed out after 60.0s"; rds.point-in-time-restore-data-query-round-trip — "Check timed out after 60.0s". Expected: the restored instance reaches available and serves the pre-RestoreTime rows.
Cause 2: rds.static-parameter-pending-reboot-query-effect — "Check timed out after 60.0s". Expected: pending-reboot status, then the new value readable after RebootDBInstance.
Cause 3: TimeoutError: 'sim2real-marker-<uuid>' in the RDS log: the run budget ended first; not graded. Expected: DownloadDBLogFilePortion's LogFileData contains the executed statement.
Cause 4: AssertionError: EnabledCloudwatchLogsExports on <instance> listing 'postgresql' was not observed within ~32s; last saw []. Expected: ['postgresql'] after ModifyDBInstance(CloudwatchLogsExportConfiguration={'EnableLogTypes': ['postgresql']}).
Cause 5: AssertionError: instance was not 'available' once the accepted password change cleared as applied: 'modifying'.
Cause 6: AssertionError: the modify response must still report the current class: 'db.t3.small' — i.e. DBInstanceClass already reads the requested db.t3.small while the live class is db.t3.micro and the change was requested with ApplyImmediately=false.
Cause 7: AssertionError: stack <name> reached 'ROLLBACK_IN_PROGRESS' waiting for 'CREATE_COMPLETE': ['<name>: The following resource(s) failed to create: [DBInstance]. ', 'DBInstance: Resource handler returned message: "The parameter MasterUserPassword is not a valid password because it is longer than 41 characters." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)']. The template's AdminSecret generates a 24-character password (GenerateSecretString PasswordLength=24) and the DBInstance references it with Fn::Sub '{{resolve:secretsmanager:${AdminSecret}:SecretString:password}}'; AWS reaches CREATE_COMPLETE.

## Steps to Reproduce
Cause 1: create a DB instance with BackupRetentionPeriod>=1, commit a row, record the time, commit another row, then RestoreDBInstanceToPointInTime(SourceDBInstanceIdentifier=<src>, TargetDBInstanceIdentifier=<new>, RestoreTime=<recorded>) and poll DescribeDBInstances for the new instance to reach available.
Cause 2: CreateDBParameterGroup, ModifyDBParameterGroup with a static parameter and ApplyMethod='pending-reboot', attach it with ModifyDBInstance, read DescribeDBParameters' ParameterApplyStatus, call RebootDBInstance, poll back to available, then read the value from a session.
Cause 3: create an instance, execute a statement containing a unique marker, then poll DescribeDBLogFiles and DownloadDBLogFilePortion for that marker.
Cause 4: create a postgres instance, call ModifyDBInstance(CloudwatchLogsExportConfiguration={'EnableLogTypes': ['postgresql']}, ApplyImmediately=True), then poll DescribeDBInstances for EnabledCloudwatchLogsExports to list 'postgresql'; then DisableLogTypes and poll for it to disappear.
Cause 5: on an available instance call ModifyDBInstance(MasterUserPassword=<a valid strong password>, ApplyImmediately=True) and poll DescribeDBInstances until DBInstanceStatus returns to 'available'.
Cause 6: on an available db.t3.micro instance call ModifyDBInstance(DBInstanceClass='db.t3.small', ApplyImmediately=False) and read the response's DBInstanceClass (must still be db.t3.micro) and PendingModifiedValues.DBInstanceClass (must be db.t3.small).
Cause 7: CreateStack with a template containing an AWS::SecretsManager::Secret using GenerateSecretString(GenerateStringKey='password', PasswordLength=24) and an AWS::RDS::DBInstance whose MasterUsername/MasterUserPassword are Fn::Sub '{{resolve:secretsmanager:${AdminSecret}:SecretString:username}}' / ':password'; wait for CREATE_COMPLETE and read the stack events.
