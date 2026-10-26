Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.delete-nonexistent-subnet-group
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.delete-nonexistent-subnet-group|run#13|AssertionError|assert msg == f"DB subnet group '{ghost}' does not exist.", msg
Issue-Commit: a348f7a60ca5edb956d11f71c3f040d86e771b20

# DeleteDBSubnetGroup's not-found message reuses the read-path template instead of the mutating-path one

> _Note: this issue was identified against the code base at commit `a348f7a60ca5edb956d11f71c3f040d86e771b20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDBSubnetGroup

## Description
ONE cause.

Cause 1 of 1: DeleteDBSubnetGroup for a DB subnet group that does not exist answers the correct fault but with the DescribeDBSubnetGroups message template, so the read and the mutating paths are indistinguishable by message. Failing check: rds.delete-nonexistent-subnet-group.

AWS: Smithy models DeleteDBSubnetGroup with DBSubnetGroupNotFoundFault (errorKind client, httpError 404, awsQueryError code DBSubnetGroupNotFoundFault / 404); its documentation is only "DBSubnetGroupName doesn't refer to an existing DB subnet group.", so the wording itself is recorded behavior only. The recorded behavior is that the message is built from the request - it quotes the requested DBSubnetGroupName - and that the MUTATING paths (DeleteDBSubnetGroup, ModifyDBSubnetGroup) use a different template from the read path: the mutating template is "DB subnet group '<DBSubnetGroupName>' does not exist." while DescribeDBSubnetGroups answers "DB Subnet group '<DBSubnetGroupName>' not found." (example, for the ghost group the check names: `DB subnet group 'sim2real-behav-sng-ghost-40279ac42f61' does not exist.`). Status 404 and code DBSubnetGroupNotFoundFault are the requirement; the per-path template parameterized by the requested name is the rule under test, the concrete group name is only an example.

Emulator: the read path is now correct (the check's DescribeDBSubnetGroups assertion on "DB Subnet group '<name>' not found." passes), but DeleteDBSubnetGroup returns HTTP 404 / DBSubnetGroupNotFoundFault carrying that SAME read template - observed `DB Subnet group 'sim2real-behav-sng-ghost-40279ac42f61' not found.` where AWS answers `DB subnet group 'sim2real-behav-sng-ghost-40279ac42f61' does not exist.` The handler builds the message from the shared helper `crate::state::db_subnet_group_not_found_message(&name)` that the describe path also uses, so both paths can only ever produce one string.

Why: a client, a CLI transcript diff, or a log-scraping/alerting rule that distinguishes 'this group was never created' (read) from 'this group cannot be mutated' (delete/modify) by the service's wording sees one undifferentiated string, and any comparison against AWS's mutating-path wording fails even though the status and code match.

Fix: the not-found message construction in the DeleteDBSubnetGroup handler (and the sibling ModifyDBSubnetGroup handler) must use the mutating-path template rather than the shared read-path helper; the describe path keeps the template it has now.

Related but not covering this: open issue 20261024T120544Z-rds-db-subnet-group-fidelity-wrong-not-found.md described the read-path template as its Cause 1; the read half now passes and only the mutating half of that wording survives, under a different failing assertion than that issue's recorded replay key, so the check is unowned. The fixed issue 20261024T030130Z-rds-deletedbsubnetgroup-stub.md implemented the operation itself - the operation does answer, so this is not a re-opened refusal.

Traces: rds.delete-nonexistent-subnet-group.

## Evidence
rds.delete-nonexistent-subnet-group (mismatch):
  File "check_rds_delete_nonexistent_subnet_group.py", line 116, in run
    assert msg == f"DB subnet group '{ghost}' does not exist.", msg
  AssertionError: DB Subnet group 'sim2real-behav-sng-ghost-40279ac42f61' not found.

Expected (AWS, recorded): HTTP 404, Error.Code DBSubnetGroupNotFoundFault, Error.Message "DB subnet group '<DBSubnetGroupName>' does not exist."
Actual (emulator): HTTP 404, Error.Code DBSubnetGroupNotFoundFault (matches), Error.Message "DB Subnet group '<DBSubnetGroupName>' not found." - the DescribeDBSubnetGroups template.

The code assertion immediately above (`assert code == "DBSubnetGroupNotFoundFault"`) and the earlier DescribeDBSubnetGroups message assertion both pass, so the divergence is isolated to the mutating path's template.

Source: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_db_subnet_group.rs lines 197-206 construct DBSubnetGroupNotFoundFault with `message: Some(crate::state::db_subnet_group_not_found_message(&name))`, the same helper the describe path uses.

Smithy: DeleteDBSubnetGroup errors = DBSubnetGroupNotFoundFault (client, httpError 404, awsQueryError DBSubnetGroupNotFoundFault/404), InvalidDBSubnetGroupStateFault (400), InvalidDBSubnetStateFault (400); DBSubnetGroupNotFoundFault doc: "DBSubnetGroupName doesn't refer to an existing DB subnet group."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral check rds.delete-nonexistent-subnet-group.
2. Create a VPC with two subnets in distinct AZs (EC2) and CreateDBSubnetGroup over them, then DeleteDBSubnetGroup it (positive control, succeeds).
3. Call Action=DescribeDBSubnetGroups with the now-deleted name: HTTP 404 / DBSubnetGroupNotFoundFault with message "DB Subnet group '<name>' not found." (correct).
4. Call Action=DeleteDBSubnetGroup with a name that was never created: the response is HTTP 404 with the correct code DBSubnetGroupNotFoundFault but message "DB Subnet group '<name>' not found." instead of AWS's mutating-path "DB subnet group '<name>' does not exist.".
5. Call Action=ModifyDBSubnetGroup on the same absent name and observe the same read-path template.
