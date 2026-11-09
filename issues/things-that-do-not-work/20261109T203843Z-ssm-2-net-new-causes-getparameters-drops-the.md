Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.delete-parameters-batch-kills-every-version-and-label-for-batch-reads ssm.session-resume-within-timeout-succeeds
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.delete-parameters-batch-kills-every-version-and-label-for-batch-reads|run#17|AssertionError|assert before == ({**want, kept: (1, "survivor")}, []), (
Issue-Key: ssm.session-resume-within-timeout-succeeds|_ping_probe#2|Timeout|return infos[0] if infos else None
Issue-Commit: 9354063c6b655c6b4143ce2300841d23d0deff44

# ssm: 2 net-new causes — GetParameters drops the ':' from the Selector it echoes, and an EC2 instance launched with AmazonSSMManagedInstanceCore never reaches PingStatus Online

> _Note: this issue was identified against the code base at commit `9354063c6b655c6b4143ce2300841d23d0deff44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameters

## Description
Two distinct SSM defects, each grounded in a failing behavioral check that the replay report does not mark ALREADY FILED or OWNED. 2 CAUSES.

Cause 1 of 2 — GetParameters echoes Parameter.Selector without its leading ':'.
AWS: when a caller asks for a parameter through a selector ('<name>:<version>' or '<name>:<label>'), the matching entry in GetParameters' Parameters list carries Parameter.Name as the bare parameter name and Parameter.Selector as the selector suffix INCLUDING the colon — ':<version>' / ':<label>' — so a batch caller can rebuild the exact string it asked with by concatenating Name + Selector. Smithy: com.amazonaws.ssm#Parameter member Selector, target PSParameterSelector @length 0..128, documented as 'Either the version number or the label used to retrieve the parameter value. Specify selectors by using one of the following formats: parameter_name:version / parameter_name:label'. The recorded behavior the check encodes is Name + Selector == the requested selector string. This call is a 200; no error code is involved.
Emulator: Selector is the bare value with the colon stripped — '1', '2', '3', '4', '<label>' — so Name + Selector collapses to '<name>1', '<name><label>'. The version and value on each entry are correct; only the echoed selector is malformed.
Why: a client that issues one GetParameters for a mixed batch of version- and label-pinned selectors and keys the response by the selector it sent cannot join a single result back to its request, and silently mis-attributes values across parameters. The ':' form is also the selector syntax AWS documents, so anything that re-feeds Selector back into a later request builds an unresolvable name.
Fix: the GetParameters (and GetParameter) response serializer — populate Parameter.Selector verbatim from the suffix of the requested name, colon included, rather than from the parsed version/label token. This is the same root cause already described as 'Cause 1 of 10' of the open issue 20261103T215115Z-ssm-parameter-store-semantics-diverge-selector-echo-filter.md; that issue does not carry this check's replay key, so this check is filed here to re-replay against the fix.
Traces: ssm.delete-parameters-batch-kills-every-version-and-label-for-batch-reads.

Cause 2 of 2 — an EC2 instance launched with an instance profile carrying AmazonSSMManagedInstanceCore never becomes a managed node, so every check that waits on registration exhausts its budget.
AWS: after RunInstances with an IamInstanceProfile whose role has arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore attached and the instance reaching the 'running' state, DescribeInstanceInformation filtered on Key='InstanceIds' returns one InstanceInformation entry for that instance and its PingStatus becomes 'Online' (Smithy: com.amazonaws.ssm#PingStatus enum values Online | ConnectionLost | Inactive). Only then do StartSession / ResumeSession / DescribeSessions have a connectable target.
Emulator: DescribeInstanceInformation keeps answering with an empty InstanceInformationList (or a non-Online PingStatus) for the instance, so the check's 420 s 'PingStatus == Online' poll never converges and the check is cut off by the harness at 180 s with no assertion ever reached.
Why: no Session Manager behavior is observable at all — a client cannot start, resume or describe a session against an instance it just launched exactly as AWS documents, and every managed-node workflow (run command, inventory, patch) is unreachable on the emulator.
Fix: the SSM managed-node registration path — an EC2 instance whose attached instance profile grants AmazonSSMManagedInstanceCore must be projected into SSM's instance-information state as an Online managed node once it is running, and DescribeInstanceInformation's InstanceIds filter must resolve it. The same cause is described by the open issue 20261107T081213Z-ssm-3-net-new-causes-a-launched-ec2.md, which the report cites as OWNER of the sibling check ssm.session-start-describe-terminate-lifecycle (identical 180 s timeout); it does not carry this check's replay key.
Traces: ssm.session-resume-within-timeout-succeeds.

## Evidence
Replay of simparity-behavioral-aws-v1; both checks are listed under '### ssm' in the suite report.

Cause 1 — [mismatch] ssm.delete-parameters-batch-kills-every-version-and-label-for-batch-reads:
AssertionError: 'before the delete GetParameters must resolve every selector exactly, got ({'/<base>/doomed1': (1, 'rev-1-x'), '/<base>/doomed2': (2, 'rev-2-xx'), '/<base>/doomed3': (3, 'rev-3-xxx'), '/<base>/doomedsim2real-pinned': (3, 'rev-3-xxx'), '/<base>/doomed4': (4, 'rev-4-xxxx'), '/<base>/doomed': (4, 'rev-4-xxxx'), '/<base>/kept': (1, 'survivor')}, [])'.
The check keys each returned entry by Parameter.Name + Parameter.Selector and asked for ['<name>:1','<name>:2','<name>:3','<name>:4','<name>:<label>','<name>','<kept>']. Expected keys are '<name>:1' ... '<name>:<label>'; the emulator produced '<name>1' ... '<name><label>', i.e. Selector == '1' where AWS returns ':1' and Selector == '<label>' where AWS returns ':<label>'. Fixed part of the requirement: the leading ':' is always present. Variable parts: the version number or label name, taken from the caller's request. Versions (1..4), values and the label-to-version-3 resolution were all correct, and InvalidParameters was correctly [].

Cause 2 — [mismatch] ssm.session-resume-within-timeout-succeeds: 'Check timed out after 180.0s'. The check's helper _create_ssm_managed_instance creates an IAM role with AmazonSSMManagedInstanceCore, an instance profile, RunInstances a t3.micro with that profile, waits for 'instance_running', then polls DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<id>]}]) for PingStatus == 'Online' with a 420 s budget; StartSession/ResumeSession/DescribeSessions are never reached. The report attributes the byte-identical 180 s timeout of the sibling check ssm.session-start-describe-terminate-lifecycle to 20261107T081213Z-ssm-3-net-new-causes-a-launched-ec2.md ('a launched ec2 instance never becomes an SSM managed node').

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (Parameter.Selector target PSParameterSelector @length 0..128 and its documentation; PingStatus enum Online|ConnectionLost|Inactive).

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the two check ids in trace_ids.
Cause 1: PutParameter '<name>' four times with Overwrite (Versions 1..4), LabelParameterVersion ParameterVersion=3 Labels=['<label>'], then GetParameters(Names=['<name>:1','<name>:2','<name>:3','<name>:4','<name>:<label>','<name>']) and inspect each returned Parameter.Selector — it must be ':1', ':2', ':3', ':4', ':<label>' and absent for the unselected name.
Cause 2: CreateRole with an ec2.amazonaws.com trust policy, AttachRolePolicy arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, CreateInstanceProfile + AddRoleToInstanceProfile, RunInstances a t3.micro with IamInstanceProfile={'Name': <profile>}, wait for 'running', then DescribeInstanceInformation(Filters=[{'Key':'InstanceIds','Values':[<instance-id>]}]) and assert one entry with PingStatus 'Online'; then StartSession(Target=<instance-id>), DescribeSessions(State='Active', Filters=[{'key':'SessionId','value':<id>}]) and ResumeSession(SessionId=<id>).
