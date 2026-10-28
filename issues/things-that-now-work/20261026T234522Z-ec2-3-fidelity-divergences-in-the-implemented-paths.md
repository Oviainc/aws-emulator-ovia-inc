Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis
Issue-Key: ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected|_legs#4|AssertionError|assert err.get("Message") == _LIMIT_MESSAGE, f"unexpected message: {err.get('Message')!r}"
Issue-Key: ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action|_error_code#23|AssertionError|raise AssertionError(f"call expected to be refused succeeded: {resp!r}")
Issue-Key: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis|run#13|AssertionError|assert m["PrefixListArn"] == f"arn:aws:ec2:{ctx.region}:aws:prefix-list/{m['PrefixListId']}", (
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# EC2: 3 fidelity divergences in the implemented paths — AuthorizeSecurityGroupIngress quota message, CreateFlowLogs accepts an unknown LogFormat field, and an AWS-managed prefix list's ARN carries the caller's account id

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Three net-new root causes in the EC2 emulator, all grounded in failing behavioral checks. None of them is a stub refusal: in this replay every operation below answered 200 (or the correct 400 code) and returned a real record, so these are fidelity defects in paths that ARE implemented.

Not covered by the open issues that look adjacent, and named here so they can be fixed together if they share a helper:
  * 20261024T081028Z-ec2-error-semantics-5-rejection-message-templates-diverge.md collects five EC2 rejection-message template divergences but does NOT carry this check's replay key and never names the rules-per-security-group quota — Cause 1 is a sixth instance of the same class in a different handler.
  * 20261026T032628Z-ec2-flow-logs-a-cloud-watch-logs-flow.md and 20261026T031112Z-ec2-a-cloud-watch-logs-vpc-flow-log-reports.md cover the synthesized LogDestination ARN and the missing S3 bucket-policy side effect. Cause 2 is a missing INPUT VALIDATION on CreateFlowLogs and is independent of both.
  * 20261022T235108Z-ec2-ipam-byoasn-routing-policy-registration-and-prefix.md covers 501 NotImplemented answers on the IPAM/prefix-list-resolver operations. Cause 3 is in DescribeManagedPrefixLists, which answers 200 here.

Cause 1 of 3: AuthorizeSecurityGroupIngress refuses the over-limit rule with the right code but the wrong message.
AWS: recorded behavior only — EC2 uses the ec2Query protocol and models no error shapes (Smithy: AuthorizeSecurityGroupIngress declares errors: [], so the code/message pair is not in the model). Authorizing one more inbound rule than the account's reported rules-per-security-group quota allows is refused HTTP 400 with error code RulesPerSecurityGroupLimitExceeded and the message "The maximum number of rules per security group has been reached." — that message is ENTIRELY fixed text, ending in a period, with NO variable part: the quota is adjustable per account (Service Quotas, ServiceCode "vpc", QuotaName containing "rules per security group"), and AWS does not name its current value in the message.
Emulator: the code is correct (RulesPerSecurityGroupLimitExceeded) but the message is "The maximum number of rules per security group has been reached (60)" — AWS's trailing period is dropped and the emulator's own limit value is appended in parentheses. The limit is read out of the emulator's state, so the parenthesised number varies with the account/limit and is not itself the requirement; the requirement is that no number appears at all.
Why: a client or harness that matches AWS's documented text to distinguish a quota refusal from another InvalidParameter-family refusal does not match, and the emulator discloses a limit value in a message AWS keeps value-free — so a caller that parses the number out of it builds a dependency that breaks against real AWS and against any account with a raised quota.
Fix: the message built by the rules-per-security-group capacity refusal in the EC2 service state, reached from both authorize handlers (state::rules_per_security_group_limit_exceeded(), called from the ingress and egress handlers) — emit AWS's fixed template and stop interpolating the limit.
Traces: ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected (the assertion on the error CODE passed; only the message assertion failed). Citations: services/service-com-amazonaws-ec2-amazonec2/src/ops/authorize_security_group_ingress.rs:314-316 calls state::rules_per_security_group_limit_exceeded(); the same helper is used by the egress path.

Cause 2 of 3: CreateFlowLogs performs no validation of the field tokens in LogFormat, so a flow log naming a field that does not exist is CREATED instead of refused.
AWS: recorded behavior only (Smithy models CreateFlowLogs$LogFormat as a plain String with no pattern and CreateFlowLogs declares errors: [], so the token vocabulary is a service-side rule). A CreateFlowLogs whose LogFormat contains a ${...} token that is not one of the documented flow-log record fields is refused HTTP 400 with error code InvalidParameter and the message "Unknown fields provided: '<field>'" — the fixed text is "Unknown fields provided: " and the variable part is the quoted name(s) of the unrecognised token(s) taken verbatim from the request's LogFormat. Nothing is created: the target resource carries exactly the flow logs it carried before the call.
Emulator: the request is ACCEPTED. The replay observed HTTP 200 with FlowLogIds: ['fl-...'] and Unsuccessful: [] for LogFormat "${srcaddr} ${sim2real-not-a-field}", and the flow log comes into being on the ENI (the flow-log id is a generated value and not part of the requirement; the 200 and the non-empty FlowLogIds are).
Why: this is an over-permissive accept on the validation path — the most dangerous class here. A client that relies on CreateFlowLogs to reject a bad custom format ships a flow log that AWS would never have created, and against real AWS the same deploy fails at create time instead of at delivery time. Worse, the accepted flow log then counts against the resource's flow logs and must be cleaned up, so a check or a caller that asserted "nothing was created" sees extra state.
Fix: CreateFlowLogs' input validation in the operation handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs) — parse the ${...} tokens out of LogFormat, compare them against the documented flow-log field vocabulary, and refuse the whole call with InvalidParameter naming every unknown token before any flow-log record is written.
Traces: ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action.

Cause 3 of 3: DescribeManagedPrefixLists reports an AWS-managed prefix list's ARN with the caller's account id in the account segment instead of the literal "aws".
AWS: Smithy (com-amazonaws-ec2-amazonec2) documents ManagedPrefixList$PrefixListArn as "The Amazon Resource Name (ARN) for the prefix list" and the operation as describing "your managed prefix lists and any Amazon Web Services-managed prefix lists"; the ARN format itself is recorded behavior. For a service-managed list such as com.amazonaws.<region>.s3, the ARN is arn:aws:ec2:<region>:aws:prefix-list/<prefix-list-id> — the account segment is the literal string "aws", which is the same marker the record carries in OwnerId ("AWS"), and it is what says the list belongs to the service rather than to the account. Region and prefix-list id are values taken from the request's Region and the resolved list; the literal "aws" account segment is the requirement.
Emulator: the record is otherwise right — OwnerId "AWS", State "create-complete", AddressFamily "IPv4" and the id agreement with DescribePrefixLists all matched — but PrefixListArn is built with the caller's account id, e.g. 'arn:aws:ec2:us-west-2:000000000000:prefix-list/pl-fffbb61c' (the region, account and id shown are one run's values; the defect is the account segment being the caller's account at all).
Why: the ARN is how a caller tells a service-managed prefix list from one it owns, and it is what goes into IAM policy Resource elements and into tooling that re-derives ownership from the ARN. A list that advertises the caller's account id as its owner in the ARN while reporting OwnerId "AWS" is self-contradictory: a policy written from that ARN does not match the real resource, and code that infers "customer-managed, therefore modifiable" from the account segment attempts ModifyManagedPrefixList on a list AWS owns.
Fix: the projection of a Region's published service prefix lists into the ManagedPrefixList view in the EC2 service state — the to_managed_prefix_list(&region) conversion used for the published lists must emit the literal "aws" account segment, not the lease/caller account. The handler itself needs no change.
Traces: ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis. Citations: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_managed_prefix_lists.rs:252-256 extends the candidates with state::service_prefix_lists_in_region(&region) mapped through to_managed_prefix_list(&region), which is where the ARN is built.

## Evidence
Cause 1 — check ec2.security-group-rules-quota-is-enforced-per-direction-at-the-accounts-own-limit-rejected [mismatch]:
  AssertionError: unexpected message: 'The maximum number of rules per security group has been reached (60)'
  Expected message: 'The maximum number of rules per security group has been reached.' (fixed text, trailing period, no limit value).
  Actual: same text with the period replaced by ' (60)'. The preceding assertion on the error code passed, so HTTP 400 / RulesPerSecurityGroupLimitExceeded is already correct, as is the per-direction bucket accounting that got the check to this beat (exactly `limit` IPv4 inbound rules were accepted and rule limit+1 was refused).

Cause 2 — check ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action [mismatch]:
  AssertionError: call expected to be refused succeeded: {'FlowLogIds': ['fl-00000000000000037'], 'Unsuccessful': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Request: CreateFlowLogs(ResourceIds=[<eni>], ResourceType='NetworkInterface', TrafficType='ALL', LogDestinationType='cloud-watch-logs', LogGroupName=<group>, DeliverLogsPermissionArn=<role>, LogFormat='${srcaddr} ${sim2real-not-a-field}', MaxAggregationInterval=60)
  Expected: HTTP 400, error code 'InvalidParameter', message "Unknown fields provided: 'sim2real-not-a-field'", and the ENI still carrying only the three flow logs created before the call.
  Actual: HTTP 200, a fourth flow log created. The flow-log id and the request id above are per-run values, not requirements.

Cause 3 — check ec2__lambda__s3.managed-prefix-list-id-agrees-across-both-describe-apis [mismatch]:
  AssertionError: the ARN reads 'arn:aws:ec2:us-west-2:000000000000:prefix-list/pl-fffbb61c'; an AWS-managed list's ARN carries the literal account segment 'aws', not the caller's account id
  Expected: PrefixListArn == 'arn:aws:ec2:<region>:aws:prefix-list/<PrefixListId>' for com.amazonaws.<region>.s3.
  Actual: the account segment is the caller's account id. Everything asserted before it matched: the two describe APIs agree on PrefixListId and PrefixListName, OwnerId == 'AWS', State == 'create-complete', AddressFamily == 'IPv4'. The region, account id and pl- id shown are one run's values.

## Steps to Reproduce
0. git_clone(), then run_behavioral_suite().

Cause 1 (AuthorizeSecurityGroupIngress quota message):
1. service-quotas ListServiceQuotas(ServiceCode='vpc') and read the quota whose name contains 'rules per security group' — call its value L (never pin 60; it is adjustable).
2. ec2 CreateVpc(CidrBlock=10.78.0.0/16) and CreateSecurityGroup in it.
3. AuthorizeSecurityGroupIngress with L distinct /32 CIDRs on one TCP port (bulk for L-1, then one more call for the last), and confirm DescribeSecurityGroupRules counts exactly L inbound rules.
4. AuthorizeSecurityGroupIngress one further distinct /32 on the same port -> expect HTTP 400, code RulesPerSecurityGroupLimitExceeded, message exactly 'The maximum number of rules per security group has been reached.'; observe the message with ' (60)' appended and no period.

Cause 2 (CreateFlowLogs LogFormat validation):
1. ec2 CreateVpc + CreateSubnet + CreateSecurityGroup + CreateNetworkInterface; logs CreateLogGroup(<group>); iam CreateRole for vpc-flow-logs.amazonaws.com.
2. ec2 CreateFlowLogs(ResourceIds=[<eni>], ResourceType='NetworkInterface', TrafficType='ALL', LogDestinationType='cloud-watch-logs', LogGroupName=<group>, DeliverLogsPermissionArn=<role arn>, LogFormat='${srcaddr} ${not-a-field}') -> expect 400 InvalidParameter "Unknown fields provided: 'not-a-field'"; observe 200 with a FlowLogIds entry.
3. ec2 DescribeFlowLogs(Filters=[{Name:'resource-id',Values:[<eni>]}]) -> the refused call must have left no flow log behind; observe the extra flow log.

Cause 3 (AWS-managed prefix list ARN):
1. ec2 DescribeManagedPrefixLists(Filters=[{Name:'prefix-list-name', Values:['com.amazonaws.<region>.s3']}]).
2. Read PrefixLists[0]: OwnerId is 'AWS' (correct) but PrefixListArn's account segment is the caller's account id; expect the literal 'aws' there, i.e. 'arn:aws:ec2:<region>:aws:prefix-list/<PrefixListId>'.
