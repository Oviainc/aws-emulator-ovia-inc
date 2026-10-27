Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.authorize-ingress-invalid-cidr-format-rejected rds.duplicate-ingress-rule-second-call-rejected rds.e2e-instance-lifecycle rds.e2e-cross-region-snapshot-copy-restore rds.e2e-shard-group-provision-and-teardown-flow rds.deploy-rds-proxy-cfn-routes-to-target
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.authorize-ingress-invalid-cidr-format-rejected|run#13|AssertionError|assert auth_code == "InvalidParameterValue", f"unexpected error code: {auth_code!r}"
Issue-Key: rds.duplicate-ingress-rule-second-call-rejected|run#12|AssertionError|assert first_code == "InvalidParameterValue", (
Issue-Key: rds.e2e-instance-lifecycle|run#31|Timeout|time.sleep(30)
Issue-Key: rds.e2e-cross-region-snapshot-copy-restore|_pg_exec_retry#19|Timeout|time.sleep(10)
Issue-Key: rds.e2e-shard-group-provision-and-teardown-flow|_wait_shard_group_status#24|Timeout|time.sleep(poll_s)
Issue-Key: rds.deploy-rds-proxy-cfn-routes-to-target|_deploy_cfn#21|Timeout|cfn.get_waiter("stack_create_complete").wait(
Issue-Commit: 4bfacb2b54a1c65638c52b387b9afadee41617eb

# RDS: AuthorizeDBSecurityGroupIngress answers 501 NotImplemented instead of AWS's InvalidParameterValue VPC refusal, and no multi-step instance/cluster lifecycle ever settles inside the check budget (2 causes)

> _Note: this issue was identified against the code base at commit `4bfacb2b54a1c65638c52b387b9afadee41617eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeDBSecurityGroupIngress

## Description
Two net-new root causes in the RDS emulator, both grounded in failing behavioral checks of this suite. CLOSE BOTH before submitting.

=== Cause 1 of 2: AuthorizeDBSecurityGroupIngress refuses every call with the scaffold's NotImplemented instead of RDS's VPC-group refusal ===
Failing checks: rds.authorize-ingress-invalid-cidr-format-rejected, rds.duplicate-ingress-rule-second-call-rejected

AWS: CreateDBSecurityGroup succeeds on a current (EC2-VPC-only) account and hands back a VPC-flavored DB security group; this legacy API version then refuses ANY ingress mutation on such a group. Smithy: AuthorizeDBSecurityGroupIngress takes DBSecurityGroupName (required) + CIDRIP/EC2SecurityGroupName/EC2SecurityGroupId/EC2SecurityGroupOwnerId and models AuthorizationAlreadyExistsFault, AuthorizationQuotaExceededFault, DBSecurityGroupNotFoundFault and InvalidDBSecurityGroupStateFault — the VPC refusal is NOT one of them: it is the RDS query-protocol common error InvalidParameterValue at HTTP 400 (recorded behavior; both checks state the refusal was measured against real RDS, and both assert the identical code and message). The message is entirely fixed text with no variable parts: "VPC DB Security Groups cannot be modified with this API version.  Please use an API version between 2012-01-15 and 2012-10-31 to modify this group." (note the two spaces between the sentences). The refusal is blanket, not CIDR-format-specific: a syntactically invalid CIDRIP (999.999.999.999/33) and a perfectly valid one (203.0.113.0/24) are rejected identically, a repeat of the identical call is rejected identically (never AuthorizationAlreadyExistsFault), and no IPRange is ever recorded on the group.

Emulator: every call is refused with error code 'NotImplemented' — the scaffold stub's 501 — so neither the code, the status, nor the message can be observed. Replay output: "AssertionError: unexpected error code: 'NotImplemented'" and "AssertionError: expected the first CIDR ingress authorization on a VPC DB security group to be rejected with InvalidParameterValue, got 'NotImplemented'". Note CreateDBSecurityGroup itself now succeeds (the checks get past it), so this is not a reopen of the CreateDBSecurityGroup stub issue.

Why: a client that handles RDS's documented refusal classes sees a 501 server-side failure instead of a 400 client error. Botocore surfaces it as a retryable/server condition rather than a terminal InvalidParameterValue, so any caller that distinguishes "my request is wrong" from "the service is broken" takes the wrong branch, and the whole legacy DB-security-group ingress contract (blanket VPC refusal, no partial write, no AuthorizationAlreadyExistsFault) is unverifiable.

Fix: the operation's handler, services/service-com-amazonaws-rds-amazonrdsv19/src/ops/authorize_db_security_group_ingress.rs — it must answer the group's VPC-backed state with the InvalidParameterValue refusal above and leave the stored group's IPRanges untouched, rather than returning the scaffold's NotImplemented. The suite also carries an auto-filed "unimplemented scaffold stub" issue naming this same operation and file (it was attributed only to the errored check rds.concurrent-authorize-ingress-duplicate-rule-single-winner); that issue does NOT carry the two replay keys above, which is why they are recorded here — the same fix closes all three.

Traces: rds.authorize-ingress-invalid-cidr-format-rejected (mismatch), rds.duplicate-ingress-rule-second-call-rejected (mismatch).

=== Cause 2 of 2: an accepted lifecycle mutation is never observable on the next read, so every multi-step RDS workflow hangs to the 60 s check cap ===
Failing checks: rds.e2e-instance-lifecycle, rds.e2e-cross-region-snapshot-copy-restore, rds.e2e-shard-group-provision-and-teardown-flow, rds.deploy-rds-proxy-cfn-routes-to-target

AWS: an accepted CreateDBInstance / StopDBInstance / StartDBInstance / DeleteDBInstance (and the cluster and shard-group equivalents) answers with the transient status (creating / stopping / starting / deleting) and the resource then REACHES its terminal state (available / stopped / gone), which is what every client and every AWS waiter polls for — rds db_instance_available polls DescribeDBInstances, rds db_instance_deleted polls for DBInstanceNotFound. Smithy: DeleteDBInstance documents "If you request a final DB snapshot, the status of the Amazon RDS DB instance is deleting until the DB snapshot is created"; DescribeDBInstances is the only read of that status. Recorded behavior only for the settling time itself: the checks allocate real-AWS-sized budgets internally (900–1800 s) but the suite caps each check at 60 s, so the emulator must make each transition observable on the FIRST read that follows the accepted mutation.

Emulator: all four checks end with "Error: Check timed out after 60.0s" — none reaches its assertions. The emulator stores a due instant and materialises the transition only when a LATER call runs: crate::state::STATUS_TRANSITION_DELAY = Duration::from_millis(300) (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:160, with PendingStatus::in_a_moment / removal_in_a_moment at state.rs:1945-1959) and DescribeDBInstances calls store.settle_due_transitions(&region) at the top of its handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_instances.rs:175). Hypothesis: because the client's first poll lands a few tens of milliseconds after the mutation — inside the 300 ms window — it always misses, and the client then pays one full poll interval (15 s in the checks' own loops, 30 s in the botocore db_instance_available waiter the proxy check uses with WaiterConfig Delay=30). rds.e2e-instance-lifecycle alone needs four sequential settles (available, stopped, available, gone) at 15 s each, i.e. 60 s minimum, before any of its assertions can run; rds.deploy-rds-proxy-cfn-routes-to-target spends 30 s on the first missed db_instance_available poll before it even creates its stack. The same store-the-due-instant pattern sits in front of these checks on the EC2 side (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc.rs:383-385 inserts vpc_available_at = now + TRANSITION_DELAY), and each of these checks opens with ec2.get_waiter("vpc_available"), costing another 15 s.

Why: a client can never drive an RDS resource through its lifecycle against this emulator. Every create/stop/start/delete appears to be accepted and then the caller's waiter burns a full poll interval per step, so any realistic workflow — provision, write, snapshot, copy, restore, tear down — exceeds its budget and reports a timeout rather than a result. No end-to-end RDS behavior (endpoint after available, stop/start round trip, not-found after delete, shard-group topology before/after, proxy target health) is observable at all.

Fix: the service's state module — the transition clock in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs (STATUS_TRANSITION_DELAY and settle_due_transitions) and the create/stop/start/delete handlers that arm it — so that the response to the mutation still carries the transient status the real service reports while the NEXT read already reports the terminal state (or is already due to settle when it runs). The comment at state.rs:155-159 says the non-zero span is deliberate so a caller reading straight after CreateDBInstance still sees 'creating'; that intent has to be preserved while making the first subsequent poll succeed. The identical pattern in the EC2 VPC store (create_vpc.rs:383-385) is on the same critical path for all four checks and needs the same treatment.

Traces: rds.e2e-instance-lifecycle, rds.e2e-cross-region-snapshot-copy-restore, rds.e2e-shard-group-provision-and-teardown-flow, rds.deploy-rds-proxy-cfn-routes-to-target (all four: "Check timed out after 60.0s").

## Evidence
Cause 1 — AuthorizeDBSecurityGroupIngress.
Expected (real RDS, VPC-backed DB security group; asserted identically by both checks):
  HTTP 400
  Error.Code: InvalidParameterValue
  Error.Message: "VPC DB Security Groups cannot be modified with this API version.  Please use an API version between 2012-01-15 and 2012-10-31 to modify this group."
  (whole message is fixed text, no request- or state-derived parts; two spaces after the first sentence)
  DescribeDBSecurityGroups afterwards: IPRanges == [] (nothing stored), and a repeated identical call is refused with the SAME code and message, never AuthorizationAlreadyExistsFault.
Actual (emulator):
  Error.Code: NotImplemented (scaffold 501) for both the malformed CIDRIP 999.999.999.999/33 and the valid 203.0.113.0/24.
  rds.authorize-ingress-invalid-cidr-format-rejected: "AssertionError: unexpected error code: 'NotImplemented'" (check line 98)
  rds.duplicate-ingress-rule-second-call-rejected: "AssertionError: expected the first CIDR ingress authorization on a VPC DB security group to be rejected with InvalidParameterValue, got 'NotImplemented'" (check line 65)
  Positive controls in both checks pass: CreateDBSecurityGroup succeeds, DescribeDBEngineVersions and the CreateDBParameterGroup round trip succeed — only the ingress operation is refused.

Cause 2 — lifecycle never settles.
Expected: each accepted mutation's terminal state is reported by the next DescribeDBInstances / DescribeDBClusters / DescribeDBShardGroups / DescribeDBProxyTargets read, so a client's waiter completes on its first or second poll; the checks then assert endpoint/status/not-found/topology/target-health.
Actual: all four checks report "Error: Check timed out after 60.0s" with no assertion reached.
  rds.e2e-instance-lifecycle needs four settles (available -> stopped -> available -> gone) at a 15 s poll interval = 60 s floor.
  rds.deploy-rds-proxy-cfn-routes-to-target uses rds.get_waiter("db_instance_available") with WaiterConfig {Delay: 30}, so one missed first poll costs 30 s before the stack is even created.
  rds.e2e-cross-region-snapshot-copy-restore and rds.e2e-shard-group-provision-and-teardown-flow time out before their data-plane / topology assertions (the cross-region check's RequiredToolMissing branch sits AFTER its available-wait and is never reached).
Source grounding: state.rs:160 STATUS_TRANSITION_DELAY = 300 ms; state.rs:1945-1959 PendingStatus::in_a_moment / removal_in_a_moment; describe_db_instances.rs:175 settle_due_transitions on read; ec2 create_vpc.rs:383-385 vpc_available_at = now + TRANSITION_DELAY.

## Steps to Reproduce
Cause 1:
1. rds.create_db_security_group(DBSecurityGroupName=<name>, DBSecurityGroupDescription=<text>) — succeeds.
2. rds.authorize_db_security_group_ingress(DBSecurityGroupName=<name>, CIDRIP="999.999.999.999/33") and again with CIDRIP="203.0.113.0/24".
   Expect both: 400 InvalidParameterValue, message "VPC DB Security Groups cannot be modified with this API version.  Please use an API version between 2012-01-15 and 2012-10-31 to modify this group."
   Observe: error code NotImplemented.
3. rds.describe_db_security_groups(DBSecurityGroupName=<name>) — IPRanges must be [].
Replay: rds.authorize-ingress-invalid-cidr-format-rejected, rds.duplicate-ingress-rule-second-call-rejected.

Cause 2:
1. ec2.create_vpc + two subnets in two AZs, rds.create_db_subnet_group over them.
2. rds.create_db_instance(Engine="mysql", DBInstanceClass="db.t3.micro", AllocatedStorage=20, BackupRetentionPeriod=0, DBSubnetGroupName=<group>).
3. Poll rds.describe_db_instances immediately and then repeatedly: the FIRST poll after the create must already be able to report DBInstanceStatus "available" (today it reports "creating" for 300 ms and the client sleeps a whole poll interval).
4. rds.stop_db_instance -> poll for "stopped"; rds.start_db_instance -> poll for "available"; rds.delete_db_instance(SkipFinalSnapshot=True) -> poll for DBInstanceNotFound.
5. Each of the four waits must complete without a 15 s/30 s sleep, so the whole workflow fits the 60 s check budget.
Replay: rds.e2e-instance-lifecycle, rds.e2e-cross-region-snapshot-copy-restore, rds.e2e-shard-group-provision-and-teardown-flow, rds.deploy-rds-proxy-cfn-routes-to-target.
