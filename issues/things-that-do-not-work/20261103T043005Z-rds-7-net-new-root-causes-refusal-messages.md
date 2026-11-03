Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.copy-cluster-parameter-group rds.create-blue-green-deployment-contract rds.create-tenant-database-boundary rds.delete-blue-green-deployment-boundaries rds.describe-blue-green-deployments rds.describe-cluster-automated-backups rds.describe-pending-maintenance
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.copy-cluster-parameter-group|run#14|AssertionError|assert message == "Parameter group does not exist", message
Issue-Key: rds.create-blue-green-deployment-contract|run#7|AssertionError|assert message == f"Source DB instance [{source_arn}] not found", message
Issue-Key: rds.create-tenant-database-boundary|run#5|AssertionError|assert message == f"DBInstance not found: {bogus_id}", message
Issue-Key: rds.delete-blue-green-deployment-boundaries|run#5|AssertionError|assert message == f"Invalid blue-green deployment identifier: {bogus_shape}", message
Issue-Key: rds.describe-blue-green-deployments|run#7|AssertionError|assert message == f"Blue Green Deployment '{well_shaped_but_unknown.lower()}' doesn't exist.", message
Issue-Key: rds.describe-cluster-automated-backups|run#5|AssertionError|assert message == f"DBClusterAutomatedBackup for DBCluster {bogus_cluster} not found.", message
Issue-Key: rds.describe-pending-maintenance|run#9|AssertionError|assert message == f"Unrecognized filter name: {bogus_filter}", message
Issue-Commit: 5e177a4f4d28f6dc4714f734dc35ed5ad192c177

# rds: 7 net-new root causes — refusal messages on copy/create/delete/describe paths are emulator wording, and DescribeBlueGreenDeployments does not lower-case the identifier it reports

> _Note: this issue was identified against the code base at commit `5e177a4f4d28f6dc4714f734dc35ed5ad192c177`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyDBClusterParameterGroup

## Description
SEVEN net-new root causes, all in the `rds` service. 7 CAUSES must be closed before this issue is submitted. Each is a separately observable refusal whose HTTP status and error CODE are already right but whose MESSAGE (or, in Cause 5, the identifier rendering inside the message) is the crate's own sentence rather than the one RDS words. None of these seven checks is marked OWNED or ALREADY FILED in the replay report.

PRIOR ART (named, not duplicated): the open issues 20261029T214632Z-rds-not-found-refusals-for-instances-clusters-subnet.md, 20261102T074545Z-rds-create-path-refusal-messages-use-emulator-wording.md and 20261102T104537Z-rds-error-envelopes-return-wrong-code-status-and.md cover the same CLASS of defect on other operations (DB instance/cluster/subnet-group lookups, create-path refusals, cluster lookup/filter envelopes). None of them carries any of the seven check ids or operations below, so those operations' messages are currently un-gated. Fix those seven operations here.

Message-template convention used below: text in plain quotes is FIXED and must appear byte-for-byte; `<Member>` marks a variable part taken from the request or from service state. No value from any one recording is part of the requirement.

Cause 1 of 7: CopyDBClusterParameterGroup with a SourceDBClusterParameterGroupIdentifier that does not exist is refused with the right code but the wrong sentence.
AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19) models CopyDBClusterParameterGroup errors DBParameterGroupAlreadyExistsFault (400), DBParameterGroupNotFoundFault (HTTP 404, awsQueryError code `DBParameterGroupNotFound`) and DBParameterGroupQuotaExceededFault (400). For an unknown SOURCE cluster parameter group RDS answers code `DBParameterGroupNotFound` with the fixed, parameter-less sentence "Parameter group does not exist" — the message has NO variable part at all and in particular does not echo the identifier that was not found (recorded behavior; asserted verbatim by the check).
Emulator: HTTP 404 code `DBParameterGroupNotFound`, message "DBClusterParameterGroup not found: <SourceDBClusterParameterGroupIdentifier>" — a different sentence that additionally leaks the requested identifier.
Why: a client that branches on, logs or matches the refusal text of a copy against a missing source group (a very common bootstrap-ordering error) never sees AWS's sentence, and tooling that asserts the parameter-less message mis-reports the failure.
Fix: the CopyDBClusterParameterGroup handler's source-group resolution (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/copy_db_cluster_parameter_group.rs) and whichever shared not-found message builder in src/state.rs it calls — the source leg must use RDS's fixed sentence, not a formatted one.
Failing check: rds.copy-cluster-parameter-group.

Cause 2 of 7: CreateBlueGreenDeployment whose Source is a well-formed DB instance ARN for an instance that does not exist words the refusal as a bare instance lookup.
AWS: Smithy models CreateBlueGreenDeployment input Source as DatabaseArn (required, length 1..2048, pattern ^arn:[A-Za-z][0-9A-Za-z-:._]*$) and models DBInstanceNotFoundFault (HTTP 404, awsQueryError code `DBInstanceNotFound`). Because the member is an ARN naming the SOURCE of the deployment, RDS words the refusal with the source framing and brackets the ARN exactly as the request passed it: "Source DB instance [" + <Source> + "] not found" — fixed text "Source DB instance [", "] not found"; the variable part is the request's Source ARN, and there is no trailing period.
Emulator: HTTP 404 code `DBInstanceNotFound`, message "DBInstance <Source> not found." — the generic instance-lookup sentence, with no "Source" framing, no brackets, and a trailing period.
Why: the message is the only thing distinguishing "the blue/green SOURCE you named is missing" from "some DB instance is missing" in a call that takes no DBInstanceIdentifier at all; operators and automation reading the refusal cannot tell which member was at fault.
Fix: the CreateBlueGreenDeployment handler's Source resolution (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_blue_green_deployment.rs) — it must build its own source-framed message instead of reusing the shared DB-instance not-found sentence.
Failing check: rds.create-blue-green-deployment-contract.

Cause 3 of 7: CreateTenantDatabase on a DB instance that does not exist words the refusal with the identifier in the wrong position.
AWS: Smithy models CreateTenantDatabase with required DBInstanceIdentifier and error DBInstanceNotFoundFault (HTTP 404, awsQueryError code `DBInstanceNotFound`). On this path RDS words it "DBInstance not found: " + <DBInstanceIdentifier> — fixed text "DBInstance not found: ", then the request's identifier, no trailing period (recorded behavior; asserted verbatim).
Emulator: HTTP 404 code `DBInstanceNotFound`, message "DBInstance <DBInstanceIdentifier> not found." — the identifier is interpolated mid-sentence and a period is appended.
Why: same class as Cause 2 — the status and code are right, so only the message distinguishes this refusal, and it does not match what AWS emits for this operation. Note the two causes require DIFFERENT sentences for the same error code, so a single shared message helper cannot satisfy both.
Fix: the CreateTenantDatabase handler's instance resolution (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_tenant_database.rs).
Failing check: rds.create-tenant-database-boundary.

Cause 4 of 7: DeleteBlueGreenDeployment's identifier-SHAPE refusal is the crate's explanatory sentence instead of RDS's fixed one.
AWS: Smithy models BlueGreenDeploymentIdentifier (length 1..255, pattern ^[A-Za-z][0-9A-Za-z-:._]*$) and models only BlueGreenDeploymentNotFoundFault (404) and InvalidBlueGreenDeploymentStateFault (400) for DeleteBlueGreenDeployment; the shape refusal is the query-protocol common error `InvalidParameterValue` (recorded behavior). RDS checks the identifier's shape BEFORE existence and words the shape refusal "Invalid blue-green deployment identifier: " + <BlueGreenDeploymentIdentifier> — fixed text, then the value from the request. (The same check also pins the EXISTENCE leg for this operation: a well-shaped unknown id must answer code `BlueGreenDeploymentNotFoundFault` with "Blue Green Deployment '" + <BlueGreenDeploymentIdentifier> + "' doesn't exist." with the identifier's case PRESERVED as the request passed it — contrast Cause 5.)
Emulator: HTTP 400 code `InvalidParameterValue`, message "invalid value '<BlueGreenDeploymentIdentifier>' for BlueGreenDeploymentIdentifier: a blue/green deployment identifier is assigned by the service and reads bgd- followed by the token it assigned" — a lower-cased generic validation sentence plus an explanation AWS never emits.
Why: clients matching the documented refusal text, and recorded-trace comparisons, see a sentence from the emulator's own validation vocabulary; the extra prose also exposes an implementation detail as if it were API contract.
Fix: the DeleteBlueGreenDeployment handler's identifier validation (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_blue_green_deployment.rs) and the shared `check_blue_green_deployment_identifier` message in src/state.rs.
Failing check: rds.delete-blue-green-deployment-boundaries.

Cause 5 of 7: DescribeBlueGreenDeployments reports the unknown identifier in the message with the request's case instead of lower-cased.
AWS: Smithy documents DescribeBlueGreenDeployments.BlueGreenDeploymentIdentifier as "This parameter isn't case-sensitive" and models BlueGreenDeploymentNotFoundFault (HTTP 404, awsQueryError code `BlueGreenDeploymentNotFoundFault`). Because the lookup is case-insensitive, RDS normalises the identifier it quotes back: the message is "Blue Green Deployment '" + lowercase(<BlueGreenDeploymentIdentifier>) + "' doesn't exist." — fixed text with the request's identifier LOWER-CASED (recorded behavior, asserted by the check's own comment "AWS names the identifier lower-cased in the message"). An unknown-but-well-shaped identifier must be REFUSED, not answered as an empty list.
Emulator: the right code and the right sentence frame, but the identifier is echoed verbatim in the case the caller sent (`crate::state::blue_green_deployment_not_found_message(requested)` is handed the raw request value at services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_blue_green_deployments.rs lines 187-201).
Why: a client comparing the message against the identifier it normalised itself (or against a recorded trace) mismatches on every identifier that is not already lower-case; worse, the un-normalised echo implies the lookup is case-sensitive when it is not.
Fix: the DescribeBlueGreenDeployments handler's not-found path (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_blue_green_deployments.rs) — lower-case the identifier when BUILDING this message only. Do NOT lower-case it in DeleteBlueGreenDeployment's not-found message, which Cause 4's check asserts preserves the case.
Failing check: rds.describe-blue-green-deployments.

Cause 6 of 7: DescribeDBClusterAutomatedBackups for an unknown DBClusterIdentifier words the refusal as prose instead of RDS's resource sentence.
AWS: Smithy models DescribeDBClusterAutomatedBackups with the single error DBClusterAutomatedBackupNotFoundFault (HTTP 404, awsQueryError code `DBClusterAutomatedBackupNotFoundFault`). The message is "DBClusterAutomatedBackup for DBCluster " + <DBClusterIdentifier> + " not found." — fixed text naming both modeled resource types, with the request's cluster identifier substituted (recorded behavior).
Emulator: HTTP 404 code `DBClusterAutomatedBackupNotFoundFault`, message "No automated backup for DB cluster <DBClusterIdentifier> was found." — an English paraphrase that names neither modeled type.
Why: the code is right, so only the message carries detail; backup tooling that matches or logs AWS's sentence (and recorded-trace comparison) diverges.
Fix: the DescribeDBClusterAutomatedBackups handler's not-found path (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_cluster_automated_backups.rs) and the message builder it calls in src/state.rs.
Failing check: rds.describe-cluster-automated-backups.

Cause 7 of 7: DescribePendingMaintenanceActions' unrecognised-Filters-name refusal is lower-cased and appends the supported-filter list.
AWS: Smithy models DescribePendingMaintenanceActions with ResourceIdentifier/Filters/Marker/MaxRecords and only ResourceNotFoundFault (404); the unrecognised-filter refusal is the query-protocol common error `InvalidParameterValue` (recorded behavior). The documented filter set for this operation is db-cluster-id and db-instance-id, and a Filters entry naming anything else is REFUSED (not silently ignored) with "Unrecognized filter name: " + <Filters[].Name> — capital U, fixed text, then the offending name from the request and NOTHING else.
Emulator: HTTP 400 code `InvalidParameterValue`, message "unrecognized filter name: <Filters[].Name>; supported filters are db-cluster-id, db-instance-id" — lower-cased first word plus a trailing enumeration AWS does not emit. The same mis-worded helper shape is visible in sibling operations (e.g. `check_filter_names` at services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_blue_green_deployments.rs lines 236-247); this cause is grounded only in the operation the failing check exercises, but the wording should be corrected wherever that helper form was copied.
Why: callers that validate their own filter names against AWS's refusal text, and any recorded-trace comparison, mismatch; the appended list is emulator-authored API surface.
Fix: the DescribePendingMaintenanceActions handler's filter-name validation (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_pending_maintenance_actions.rs).
Failing check: rds.describe-pending-maintenance.

## Evidence
Suite simparity-behavioral-aws-v1, service heading `### rds` (70/309 passed). All seven failures below are [mismatch] verdicts — the emulator answered, with the correct HTTP status and error code, and only the message diverged. None is marked OWNED or ALREADY FILED.

Cause 1 — [mismatch] rds.copy-cluster-parameter-group
  check_rds_copy_cluster_parameter_group.py line 77: assert message == "Parameter group does not exist"
  AssertionError: DBClusterParameterGroup not found: sim2real-behav-cpg-missing-7b08410195ab
  (code asserted and matched: DBParameterGroupNotFound)
  Expected (AWS): code DBParameterGroupNotFound, message exactly "Parameter group does not exist" (no variable part).
  Actual (emulator): same code, message "DBClusterParameterGroup not found: <source identifier>".

Cause 2 — [mismatch] rds.create-blue-green-deployment-contract
  check_rds_create_blue_green_deployment_contract.py line 40: assert message == f"Source DB instance [{source_arn}] not found"
  AssertionError: DBInstance arn:aws:rds:<region>:<account>:db:<missing id> not found.
  (code asserted and matched: DBInstanceNotFound)
  Expected (AWS): "Source DB instance [" + Source ARN + "] not found".
  Actual (emulator): "DBInstance " + Source ARN + " not found.".

Cause 3 — [mismatch] rds.create-tenant-database-boundary
  check_rds_create_tenant_database_boundary.py line 39: assert message == f"DBInstance not found: {bogus_id}"
  AssertionError: DBInstance <missing id> not found.
  (code asserted and matched: DBInstanceNotFound)
  Expected (AWS): "DBInstance not found: " + DBInstanceIdentifier.
  Actual (emulator): "DBInstance " + DBInstanceIdentifier + " not found.".

Cause 4 — [mismatch] rds.delete-blue-green-deployment-boundaries
  check_rds_delete_blue_green_deployment_boundaries.py line 37: assert message == f"Invalid blue-green deployment identifier: {bogus_shape}"
  AssertionError: invalid value '<bad identifier>' for BlueGreenDeploymentIdentifier: a blue/green deployment identifier is assigned by the service and reads bgd- followed by the token it assigned
  (code asserted and matched: InvalidParameterValue)
  Expected (AWS): "Invalid blue-green deployment identifier: " + the request's identifier. The same check then requires the well-shaped-unknown leg to answer BlueGreenDeploymentNotFoundFault with "Blue Green Deployment '<identifier, case preserved>' doesn't exist."

Cause 5 — [mismatch] rds.describe-blue-green-deployments
  check_rds_describe_blue_green_deployments.py line 44: assert message == f"Blue Green Deployment '{well_shaped_but_unknown.lower()}' doesn't exist." (the check comments: "AWS names the identifier lower-cased in the message")
  AssertionError: Blue Green Deployment 'bgd-73F3B5C1EFE04A64BD2719E152' doesn't exist.  <- emulator echoed the request's upper-case form
  (code asserted and matched: BlueGreenDeploymentNotFoundFault)
  Source of the echo: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_blue_green_deployments.rs lines 187-201 pass the raw `requested` value to crate::state::blue_green_deployment_not_found_message.
  Smithy: DescribeBlueGreenDeployments.BlueGreenDeploymentIdentifier — "This parameter isn't case-sensitive."; error BlueGreenDeploymentNotFoundFault HTTP 404.

Cause 6 — [mismatch] rds.describe-cluster-automated-backups
  check_rds_describe_cluster_automated_backups.py line 53: assert message == f"DBClusterAutomatedBackup for DBCluster {bogus_cluster} not found."
  AssertionError: No automated backup for DB cluster <missing cluster id> was found.
  (code asserted and matched: DBClusterAutomatedBackupNotFoundFault)

Cause 7 — [mismatch] rds.describe-pending-maintenance
  check_rds_describe_pending_maintenance.py line 40: assert message == f"Unrecognized filter name: {bogus_filter}"
  AssertionError: unrecognized filter name: sim2real-not-a-real-filter-name; supported filters are db-cluster-id, db-instance-id
  (code asserted and matched: InvalidParameterValue)

Smithy citations (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json):
  CopyDBClusterParameterGroup -> errors DBParameterGroupAlreadyExistsFault 400, DBParameterGroupNotFoundFault 404 (code DBParameterGroupNotFound), DBParameterGroupQuotaExceededFault 400.
  CreateBlueGreenDeployment -> input Source: DatabaseArn, required, len 1..2048, pattern ^arn:[A-Za-z][0-9A-Za-z-:._]*$; errors include DBInstanceNotFoundFault 404 (code DBInstanceNotFound).
  CreateTenantDatabase -> required DBInstanceIdentifier/TenantDBName/MasterUsername; errors DBInstanceNotFoundFault 404, InvalidDBInstanceStateFault 400, KMSKeyNotAccessibleFault 400, TenantDatabaseAlreadyExistsFault 400, TenantDatabaseQuotaExceededFault 400.
  DeleteBlueGreenDeployment -> BlueGreenDeploymentIdentifier len 1..255 pattern ^[A-Za-z][0-9A-Za-z-:._]*$; errors BlueGreenDeploymentNotFoundFault 404, InvalidBlueGreenDeploymentStateFault 400 (InvalidParameterValue is the protocol-common refusal).
  DescribeBlueGreenDeployments -> error BlueGreenDeploymentNotFoundFault 404; identifier documented case-insensitive.
  DescribeDBClusterAutomatedBackups -> single error DBClusterAutomatedBackupNotFoundFault 404.
  DescribePendingMaintenanceActions -> single modeled error ResourceNotFoundFault 404; the unrecognised-filter refusal is InvalidParameterValue (recorded behavior only).

## Steps to Reproduce
Build and boot the emulator from this branch, then replay the seven ids in trace_ids. Manually, against the RDS endpoint (any region, Query protocol or any SDK):
1. rds.copy_db_cluster_parameter_group(SourceDBClusterParameterGroupIdentifier=<a name that does not exist>, TargetDBClusterParameterGroupIdentifier=<new>, TargetDBClusterParameterGroupDescription="x") -> expect 404 DBParameterGroupNotFound, message exactly "Parameter group does not exist". Emulator: "DBClusterParameterGroup not found: <name>".
2. rds.create_blue_green_deployment(BlueGreenDeploymentName=<name>, Source="arn:aws:rds:<region>:<account>:db:<missing instance>") -> expect 404 DBInstanceNotFound, message "Source DB instance [<that ARN>] not found". Emulator: "DBInstance <that ARN> not found.". (The same call with Source="not-an-arn-at-all" must NOT answer DBInstanceNotFound.)
3. rds.create_tenant_database(DBInstanceIdentifier=<missing instance>, MasterUsername="admin", TenantDBName="tenant1", MasterUserPassword=<pw>) -> expect 404 DBInstanceNotFound, message "DBInstance not found: <that identifier>". Emulator: "DBInstance <identifier> not found.".
4. rds.delete_blue_green_deployment(BlueGreenDeploymentIdentifier="some-arbitrary-string") -> expect 400 InvalidParameterValue, message "Invalid blue-green deployment identifier: some-arbitrary-string". Emulator: "invalid value '...' for BlueGreenDeploymentIdentifier: a blue/green deployment identifier is assigned by the service and reads bgd- followed by the token it assigned". Then delete_blue_green_deployment(BlueGreenDeploymentIdentifier="bgd-" + 26 upper-case hex chars) -> expect 404 BlueGreenDeploymentNotFoundFault, message "Blue Green Deployment '<identifier exactly as sent>' doesn't exist.".
5. rds.describe_blue_green_deployments(BlueGreenDeploymentIdentifier="bgd-" + 26 UPPER-case hex chars) -> expect 404 BlueGreenDeploymentNotFoundFault with the identifier LOWER-CASED inside the message: "Blue Green Deployment '<identifier.lower()>' doesn't exist.". Emulator echoes the upper-case form.
6. rds.describe_db_cluster_automated_backups(DBClusterIdentifier=<a cluster that does not exist>) -> expect 404 DBClusterAutomatedBackupNotFoundFault, message "DBClusterAutomatedBackup for DBCluster <that identifier> not found.". Emulator: "No automated backup for DB cluster <identifier> was found.".
7. rds.describe_pending_maintenance_actions(Filters=[{"Name": "not-a-real-filter-name", "Values": ["x"]}]) -> expect 400 InvalidParameterValue, message "Unrecognized filter name: not-a-real-filter-name" and nothing more. Emulator lower-cases it and appends "; supported filters are db-cluster-id, db-instance-id".
