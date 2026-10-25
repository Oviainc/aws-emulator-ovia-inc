Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.flow-log-custom-format-vs-default ec2.op-contract-delete-flow-logs-batch-rejection ec2.op-contract-describeFlowLogs
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.flow-log-custom-format-vs-default|_retry_create_flow_log#12|Timeout|time.sleep(interval_s)
Issue-Key: ec2.op-contract-delete-flow-logs-batch-rejection|_retry_create_flow_log#17|Timeout|time.sleep(interval_s)
Issue-Key: ec2.op-contract-describeFlowLogs|_retry_create_flow_log#15|Timeout|time.sleep(interval_s)
Issue-Commit: 95f850a4c04866a54d7ccecba673fc8b3153145d

# ec2 flow-log contract is unreachable: the Create/Describe/DeleteFlowLogs scaffold stubs make three behavioral checks hang to the 60 s harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `95f850a4c04866a54d7ccecba673fc8b3153145d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Cause 1 of 1: the VPC flow-log surface of ec2 answers HTTP 501 NotImplemented on CreateFlowLogs, DescribeFlowLogs and DeleteFlowLogs, so three behavioral checks never get a flow log to observe and are killed by the harness at its 60 s cap instead of grading the flow-log contract. Failing checks: ec2.flow-log-custom-format-vs-default, ec2.op-contract-delete-flow-logs-batch-rejection, ec2.op-contract-describeFlowLogs.

NOTE — related open issues, none of which carries these three checks: `20261022T235108Z-ec2-createflowlogs-stub.md`, `20261023T102725Z-ec2-describeflowlogs-stub.md` and `20261023T102725Z-ec2-deleteflowlogs-stub.md` each ask for one of these operations to be implemented. The CreateFlowLogs issue's Issue-Trace-Ids list names 10 other units and NOT these three, so a fixer closing those issues would never re-replay them; this issue exists to carry their replay keys and the three relationship-level requirements below, which no single-operation stub issue states.

AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json).
(a) CreateFlowLogs.LogFormat (String) — "The fields to include in the flow log record... If you omit this parameter, the flow log is created using the default format." AWS's documented default format string is `${version} ${account-id} ${interface-id} ${srcaddr} ${dstaddr} ${srcport} ${dstport} ${protocol} ${packets} ${bytes} ${start} ${end} ${action} ${log-status}`, and DescribeFlowLogs must report it as a concrete LogFormat value (not null/empty) for a flow log created with no LogFormat, while a flow log created WITH an explicit LogFormat reports that exact string — the two must differ.
(b) DescribeFlowLogs: input FlowLogIds (FlowLogIdList), output FlowLogs (FlowLogSet) + NextToken, errors: [] — it is a filter, not a point lookup: a well-formed but nonexistent id such as `fl-0123456789abcdef0` matches nothing and the call succeeds with an empty FlowLogs list rather than raising. A real flow log must read back with FlowLogId, ResourceId (the VPC), TrafficType=ALL, LogDestinationType=cloud-watch-logs, LogGroupName and FlowLogStatus=ACTIVE, and must also be reachable through Filter Name=resource-id.
(c) DeleteFlowLogs: input FlowLogIds required, output Unsuccessful (UnsuccessfulItemSet), errors: [] in the model — recorded behavior only for the batch rule: AWS validates the whole FlowLogIds batch up front and refuses the entire call with error code `InvalidFlowLogId.NotFound` when ANY id is unknown, deleting nothing, rather than reporting the unknown id per-item in `Unsuccessful`. The message is built from the request (it enumerates the unknown ids and their count), so the rule is the requirement, e.g. `These flow log ids in the input list are not found: [TotalCount: 1] fl-00000000000000000`. The refusal must name only the unknown id(s), never the valid ones, and a valid id riding along in the same batch must survive untouched. A lone valid id deletes cleanly with an empty `Unsuccessful`. With DryRun=true the call answers `DryRunOperation` ("Request would have succeeded, but DryRun flag is set.").

Emulator: every one of the three operations is an unmodified scaffold. services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs::CreateFlowLogs::handle returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateFlowLogs"))` (HTTP 501, code `NotImplemented`, message "operation CreateFlowLogs is scaffolded but not implemented in this emulator"), and the Describe/Delete handlers do the same. All three checks build a VPC, a CloudWatch Logs log group and a flow-log delivery role, then retry CreateFlowLogs in a loop (treating the refusal as IAM-propagation lag); the loop never converges, so the replay reports `Check timed out after 60.0s` and no flow-log assertion is ever evaluated.

Why: a client cannot create, read or delete a VPC flow log at all — IaC and observability tooling that turns on flow logs gets a 501 where AWS returns FlowLogIds. Once create is implemented, the three relationship-level contracts above still have to hold or clients break differently: a consumer that reads LogFormat to learn the record layout gets null instead of the default format string, a consumer that probes DescribeFlowLogs with an id that may not exist gets an exception instead of an empty list, and a batch delete that includes one stale id either silently half-deletes (per-item Unsuccessful) or wrongly reports the valid ids as missing.

Fix: implement the three handlers and the flow-log records in the ec2 service state — CreateFlowLogs must default LogFormat to AWS's documented default when the member is absent and store the explicit string otherwise; DescribeFlowLogs must treat FlowLogIds and Filter as filters over that state (empty result for unknown ids, resource-id filtering) and serialize LogFormat/FlowLogStatus/LogDestinationType/LogGroupName/ResourceId; DeleteFlowLogs must validate the whole FlowLogIds batch before mutating anything and refuse with InvalidFlowLogId.NotFound naming the unknown ids, with DryRun handled in the operation's validation path ahead of the mutation.

Traces: ec2.flow-log-custom-format-vs-default (two flow logs on one VPC, one with an explicit LogFormat and one without, must report DIFFERENT LogFormat values); ec2.op-contract-describeFlowLogs (read-back of configuration + resource-id filter + empty result for a bogus id); ec2.op-contract-delete-flow-logs-batch-rejection (lone-valid delete, all-unknown batch rejection, mixed batch rejection with the valid id surviving, DryRun).

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1), three ec2 checks, none marked ALREADY FILED and none OWNED by an open issue:

  #### [mismatch] ec2.flow-log-custom-format-vs-default
  Error: Check timed out after 60.0s

  #### [mismatch] ec2.op-contract-delete-flow-logs-batch-rejection
  Error: Check timed out after 60.0s

  #### [mismatch] ec2.op-contract-describeFlowLogs
  Error: Check timed out after 60.0s

Each check wraps CreateFlowLogs in a bounded retry loop (`_retry_create_flow_log`, 60-90 s budget, swallowing ClientError as IAM propagation lag), which is why the 501 surfaces as a harness timeout rather than the NotImplemented ClientError the suite auto-attributes.

The same refusal, observed directly by a sibling check in this corpus:
  create_flow_logs: expected ('InvalidVpcId.NotFound', ...), got ('NotImplemented', 'operation CreateFlowLogs is scaffolded but not implemented in this emulator')
  delete_flow_logs: expected ('InvalidFlowLogId.NotFound', 'These flow log ids in the input list are not found: [TotalCount: 1] fl-0123456789abcdef0'), got ('NotImplemented', 'operation DeleteFlowLogs is scaffolded but not implemented in this emulator')

Handler source, services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs:
  line 4:   //! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`
  line 211: Err(crate::scaffolded::Scaffolded::not_implemented("CreateFlowLogs"))

Expected vs actual, per grounded check:
- flow-log-custom-format-vs-default — expected: the LogFormat-less flow log reads back LogFormat == '${version} ${account-id} ${interface-id} ${srcaddr} ${dstaddr} ${srcport} ${dstport} ${protocol} ${packets} ${bytes} ${start} ${end} ${action} ${log-status}' and the explicit one reads back '${version} ${srcaddr} ${dstaddr} ${action}', the two differing. Actual: no flow log is ever created (501), check killed at 60 s.
- op-contract-describeFlowLogs — expected: HTTP 200 with exactly one FlowLog whose ResourceId is the VPC, TrafficType=ALL, LogDestinationType=cloud-watch-logs, FlowLogStatus=ACTIVE; resource-id filter includes it; FlowLogIds=['fl-0123456789abcdef0'] returns FlowLogs == []. Actual: 501 NotImplemented on create and on describe, check killed at 60 s.
- op-contract-delete-flow-logs-batch-rejection — expected: DeleteFlowLogs of a lone valid id returns Unsuccessful == []; an all-unknown or mixed batch is refused with code 'InvalidFlowLogId.NotFound' naming only the unknown ids and deletes nothing; DryRun=true yields code 'DryRunOperation'. Actual: 501 NotImplemented, check killed at 60 s.

## Steps to Reproduce
1. Boot the emulator from this checkout (the behavioral suite's build/boot path).
2. ec2 CreateVpc CidrBlock=10.213.0.0/16; logs CreateLogGroup; iam CreateRole with a vpc-flow-logs.amazonaws.com trust policy and CloudWatchLogsFullAccess attached.
3. ec2 CreateFlowLogs ResourceType=VPC ResourceIds=[<vpc>] TrafficType=ALL LogDestinationType=cloud-watch-logs LogGroupName=<group> DeliverLogsPermissionArn=<role arn> — observe HTTP 501 with code NotImplemented instead of a FlowLogIds list. Repeat with LogFormat omitted and with LogFormat='${version} ${srcaddr} ${dstaddr} ${action}'.
4. ec2 DescribeFlowLogs FlowLogIds=['fl-0123456789abcdef0'] — observe HTTP 501 instead of HTTP 200 with FlowLogs == [].
5. ec2 DeleteFlowLogs FlowLogIds=['fl-00000000000000000','fl-11111111111111111'] — observe HTTP 501 instead of the whole-batch refusal with code InvalidFlowLogId.NotFound; and with DryRun=true observe 501 instead of DryRunOperation.
6. Re-replay the three grounded checks (ec2.flow-log-custom-format-vs-default, ec2.op-contract-describeFlowLogs, ec2.op-contract-delete-flow-logs-batch-rejection) to confirm they grade instead of timing out at 60 s.
