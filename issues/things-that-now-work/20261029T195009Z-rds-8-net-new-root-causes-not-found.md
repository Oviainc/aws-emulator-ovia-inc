Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.authorize-security-group-ingress rds.describe-db-proxies-contract rds.describe-export-tasks rds.describe-engine-versions-nonexistent rds.describe-event-categories rds.modify-db-parameter-group-invalid-value-rejected rds.reboot-instance-before-available-rejected rds.parameter-group-empty-vs-absent-description-preserved
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.authorize-security-group-ingress|run#13|AssertionError|assert message == expected, f"unexpected message: {message!r} (expected {expected!r})"
Issue-Key: rds.describe-db-proxies-contract|run#8|AssertionError|assert err.get("Message") == f"DBProxy '{bogus_proxy}' not found.", (
Issue-Key: rds|DescribeExportTasks|ExportTaskNotFound
Issue-Key: rds.describe-engine-versions-nonexistent|_rejection#4|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: rds.describe-event-categories|run#13|AssertionError|assert message.startswith(prefix), f"expected {prefix!r}..., got {message!r}"
Issue-Key: rds.modify-db-parameter-group-invalid-value-rejected|run#13|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: rds.reboot-instance-before-available-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds.parameter-group-empty-vs-absent-description-preserved|_error#4|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rds: 8 net-new root causes — not-found/invalid-value refusals are worded with the wrong templates, and four over-permissive accepts (DescribeExportTasks, DescribeDBEngineVersions, CreateDBParameterGroup)

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeDBSecurityGroupIngress

## Description
Eight distinct root causes in the rds crate's refusal wording and input validation. All are grounded in failing behavioral checks of the rds service; none is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 8: AuthorizeDBSecurityGroupIngress words its not-found refusal with the READ-path template.
AWS: Smithy: AuthorizeDBSecurityGroupIngress errors DBSecurityGroupNotFoundFault (awsQueryError code `DBSecurityGroupNotFound`, HTTP 404). On the mutating path AWS words the message as the fixed text "DBSecurityGroup not found: " followed by the DBSecurityGroupName taken from the request; the read path (DescribeDBSecurityGroups) words the SAME condition differently, as "DBSecurityGroup " + name + " not found." The check asserts both templates in one run, so the two wordings are separate requirements.
Emulator: the error code is right but the message is the read-path template: 'DBSecurityGroup <name> not found.' (recorded: 'DBSecurityGroup sim2real-behav-secg-missing-123ef449782b not found.'; the name is the request value, the rest is fixed text).
Why: a client (or a CloudFormation provider) that matches the mutating-path message cannot distinguish "this group does not exist" from any other refusal, and message-equality assertions fail.
Fix: the AuthorizeDBSecurityGroupIngress handler's refusal wording in the rds crate — it must use the mutate-path not-found template, not the shared describe template.
Traces: rds.authorize-security-group-ingress

Cause 2 of 8: DescribeDBProxies' DBProxyNotFoundFault message does not quote the proxy name.
AWS: Smithy: DescribeDBProxies errors DBProxyNotFoundFault (HTTP 404). AWS's message is the fixed text "DBProxy '" + the requested DBProxyName + "' not found." — the name is single-quoted.
Emulator: code DBProxyNotFoundFault (Type Sender) with 'DBProxy <name> not found.' — the quotes around the request-supplied name are missing (recorded: 'DBProxy sim2real-behav-proxy-missing-748824e9cace not found.').
Why: callers that parse or compare the refusal text see a message AWS never emits.
Fix: the DescribeDBProxies handler's not-found wording.
Traces: rds.describe-db-proxies-contract

Cause 3 of 8: DescribeExportTasks refuses an unknown ExportTaskIdentifier instead of answering an empty list.
AWS: Smithy models ExportTaskNotFoundFault on DescribeExportTasks, but filtering by an identifier that names no export task is a legitimate empty success: HTTP 200 with ExportTasks: [] (recorded behavior). Smithy also bounds DescribeExportTasks$MaxRecords with range min 20 / max 100, and AWS refuses a below-range value server-side with InvalidParameterValue and the message "1 validation error detected: Value '<value>' at 'maxRecords' failed to satisfy constraint: Member must have value greater than or equal to 20" (fixed text; the value comes from the request).
Emulator: DescribeExportTasks(ExportTaskIdentifier=<unknown>) raises ExportTaskNotFound 404 with 'ExportTask <id> not found.', which aborts the check before the MaxRecords boundary is ever exercised — so the MaxRecords=5 refusal and the MaxRecords=20 positive control are both still ungraded.
Why: every list-and-filter client treats a filtered describe as a query, not an existence assertion; a 404 turns an empty result into an error path.
Fix: the DescribeExportTasks handler — filtering is a predicate over the catalog, and its MaxRecords range refusal belongs in that operation's validation.
Traces: rds.describe-export-tasks

Cause 4 of 8: DescribeDBEngineVersions accepts an engine name it does not recognize.
AWS: an unrecognized Engine value is refused with InvalidParameterValue and the message "Unrecognized engine name: " + the Engine value from the request.
Emulator: HTTP 200 with {'DBEngineVersions': []} for Engine='sim2real-fake-engine' (the positive control, Engine='mysql', does return versions).
Why: a typo in an engine name silently yields an empty catalog instead of a refusal, so callers build on a nonexistent engine family.
Fix: Engine-value validation in the DescribeDBEngineVersions handler.
Traces: rds.describe-engine-versions-nonexistent

Cause 5 of 8: DescribeEventCategories words its invalid-SourceType refusal differently from AWS.
AWS: the refusal message begins with the fixed text "Invalid event source type. Feature may be disabled. Valid types are " followed by the valid-type list; it does NOT echo the offending value.
Emulator: 'Invalid event source type: not-a-real-source-type. Valid values are: db-instance, db-cluster, db-parameter-group, db-security-group, db-snapshot, db-cluster-snapshot, db-proxy' — a different fixed prefix ("Invalid event source type: " + the request value, then "Valid values are: ").
Why: the message is a requirement of the error class; clients and recorded-fidelity comparisons see text AWS never emits.
Fix: the DescribeEventCategories handler's validation message.
Traces: rds.describe-event-categories

Cause 6 of 8: ModifyDBParameterGroup's out-of-domain value refusal uses a non-AWS message template.
AWS: code InvalidParameterValue, HTTP 400, message = fixed text "Invalid parameter value: " + the rejected ParameterValue + " for: " + the ParameterName + " allowed values are: " + that parameter's AllowedValues (all three variable parts come from the request and from the parameter's own catalog entry).
Emulator: same code/status but message 'Value: not_a_valid_enum_value is outside of range: none,ddl,mod,all for parameter: log_statement' — a different template ("Value: ... is outside of range: ... for parameter: ...").
Why: the parameter-validation error message is how tooling reports which value/parameter/domain was wrong; the divergent template breaks that contract.
Fix: the ModifyDBParameterGroup handler's parameter-value validation wording.
Traces: rds.modify-db-parameter-group-invalid-value-rejected

Cause 7 of 8: RebootDBInstance's wrong-state refusal message is emulator-invented.
AWS: message = the fixed text "Can only reboot db instances with state in: available, storage-optimization, storage-initialization, incompatible-credentials, incompatible-parameters." then two spaces, then "Instance " + the DBInstanceIdentifier from the request + " has state: " + the instance's current status from service state + "."
Emulator: 'DB instance <id> is not available for a reboot (status creating)' — a different template that neither lists the permitted states nor matches AWS's sentence structure.
Why: the permitted-state list in AWS's message is what a caller uses to decide whether to wait or to fail; the emulator's wording drops it.
Fix: the RebootDBInstance handler's state-gate refusal wording.
Traces: rds.reboot-instance-before-available-rejected

Cause 8 of 8: CreateDBParameterGroup accepts an empty Description.
AWS: recorded behavior only — Description is a required member of CreateDBParameterGroupMessage and AWS refuses a create whose Description is the empty string (the check treats the call as one AWS rejects).
Emulator: HTTP 200, and the group is created and read back with Description: '' (recorded result shows DBParameterGroupName/Family set and Description '').
Why: an over-permissive accept lets a client create state AWS would never create, and the difference only surfaces later when the same code runs against AWS.
Fix: CreateDBParameterGroup input validation.
Traces: rds.parameter-group-empty-vs-absent-description-preserved

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-aws-v1).

1) rds.authorize-security-group-ingress [mismatch]: AssertionError: unexpected message: 'DBSecurityGroup sim2real-behav-secg-missing-123ef449782b not found.' (expected 'DBSecurityGroup not found: sim2real-behav-secg-missing-123ef449782b')

2) rds.describe-db-proxies-contract [mismatch]: AssertionError: unexpected DBProxyNotFoundFault message: {'Type': 'Sender', 'Code': 'DBProxyNotFoundFault', 'Message': 'DBProxy sim2real-behav-proxy-missing-748824e9cace not found.'} — check asserts Message == "DBProxy '<name>' not found."

3) rds.describe-export-tasks [error]: botocore.errorfactory.ExportTaskNotFoundFault: An error occurred (ExportTaskNotFound) when calling the DescribeExportTasks operation: ExportTask sim2real-behav-exporttask-missing-1e37ada5fb9b not found. Expected: ExportTasks == [] for an unknown identifier; then InvalidParameterValue for MaxRecords=5 with "1 validation error detected: Value '5' at 'maxRecords' failed to satisfy constraint: Member must have value greater than or equal to 20"; MaxRecords=20 accepted.

4) rds.describe-engine-versions-nonexistent [mismatch]: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'DBEngineVersions': [], ... 'HTTPStatusCode': 200 ...}. Expected code InvalidParameterValue, message 'Unrecognized engine name: sim2real-fake-engine'.

5) rds.describe-event-categories [mismatch]: AssertionError: expected 'Invalid event source type. Feature may be disabled. Valid types are '..., got 'Invalid event source type: not-a-real-source-type. Valid values are: db-instance, db-cluster, db-parameter-group, db-security-group, db-snapshot, db-cluster-snapshot, db-proxy'

6) rds.modify-db-parameter-group-invalid-value-rejected [mismatch]: AssertionError: expected the message 'Invalid parameter value: not_a_valid_enum_value for: log_statement allowed values are: none,ddl,mod,all', got 'Value: not_a_valid_enum_value is outside of range: none,ddl,mod,all for parameter: log_statement' (code/status InvalidParameterValue/400 did match).

7) rds.reboot-instance-before-available-rejected [mismatch]: AssertionError: expected message 'Can only reboot db instances with state in: available, storage-optimization, storage-initialization, incompatible-credentials, incompatible-parameters.  Instance sim2real-rds-rbt-9f48253e7e2c has state: creating.', got 'DB instance sim2real-rds-rbt-9f48253e7e2c is not available for a reboot (status creating)'

8) rds.parameter-group-empty-vs-absent-description-preserved [mismatch]: AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'DBParameterGroup': {'DBParameterGroupName': 'sim2real-pgdesc-empty-d4e5561dc087', 'DBParameterGroupFamily': 'mysql8.4', 'Description': '', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Smithy citations (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): AuthorizeDBSecurityGroupIngress error DBSecurityGroupNotFoundFault / awsQueryError DBSecurityGroupNotFound 404; DescribeDBProxies error DBProxyNotFoundFault 404; DescribeExportTasks$MaxRecords range min 20 max 100, error ExportTaskNotFoundFault 404.

## Steps to Reproduce
Build and boot the emulator, then replay these behavioral checks:
  rds.authorize-security-group-ingress
  rds.describe-db-proxies-contract
  rds.describe-export-tasks
  rds.describe-engine-versions-nonexistent
  rds.describe-event-categories
  rds.modify-db-parameter-group-invalid-value-rejected
  rds.reboot-instance-before-available-rejected
  rds.parameter-group-empty-vs-absent-description-preserved

Manually, against the emulator's rds endpoint:
1. rds.authorize_db_security_group_ingress(DBSecurityGroupName=<nonexistent>, CIDRIP='10.0.0.0/32') -> expect DBSecurityGroupNotFound with 'DBSecurityGroup not found: <name>'.
2. rds.describe_db_proxies(DBProxyName=<nonexistent>) -> expect DBProxyNotFoundFault with "DBProxy '<name>' not found."
3. rds.describe_export_tasks(ExportTaskIdentifier=<nonexistent>) -> expect 200 with ExportTasks == []; then, with botocore parameter validation disabled, describe_export_tasks(MaxRecords=5) -> expect InvalidParameterValue, and MaxRecords=20 -> accepted.
4. rds.describe_db_engine_versions(Engine='sim2real-fake-engine', MaxRecords=20) -> expect InvalidParameterValue 'Unrecognized engine name: sim2real-fake-engine'.
5. rds.describe_event_categories(SourceType='not-a-real-source-type') -> expect the 'Invalid event source type. Feature may be disabled. Valid types are ...' message.
6. Create a postgres16 DB parameter group, then modify_db_parameter_group(Parameters=[{log_statement: 'not_a_valid_enum_value', ApplyMethod: immediate}]) -> expect the 'Invalid parameter value: ... for: ... allowed values are: ...' message.
7. create_db_instance(...) and immediately reboot_db_instance(...) while status is 'creating' -> expect AWS's 'Can only reboot db instances with state in: ...  Instance <id> has state: <status>.' message.
8. create_db_parameter_group(DBParameterGroupName=<new>, DBParameterGroupFamily='mysql8.4', Description='') -> expect a rejection, not a 200.
