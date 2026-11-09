Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.dasgrefresh-instance-refresh-replaces-every-guest-with-the-new-launch-template-version ec2.op-contract-describe-reserved-instances-offerings
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.dasgrefresh-instance-refresh-replaces-every-guest-with-the-new-launch-template-version|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: ec2.op-contract-describe-reserved-instances-offerings|run#9|AssertionError|assert withdrawn == [], (
Issue-Commit: 28a8a61701b6d4c7fe3b061a21d38872ad20994f

# ec2: 2 net-new causes — the instance-refresh-replaces-every-guest flow never converges, and the m5.large standard RI catalog is served instead of withdrawn

> _Note: this issue was identified against the code base at commit `28a8a61701b6d4c7fe3b061a21d38872ad20994f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceRefresh

## Description
2 root causes, grounded in 2 failing checks.

Cause 1 of 2 (check: ec2.dasgrefresh-instance-refresh-replaces-every-guest-with-the-new-launch-template-version): an instance refresh that must replace every guest with a new launch-template version never converges, so the check reaches no verdict.

AWS: publishing a new launch-template version, pointing an Auto Scaling group of desired capacity 2 at it and calling StartInstanceRefresh with MinHealthyPercentage 100 replaces both instances; the refresh reaches a terminal status (Successful / Failed / Cancelled / RollbackSuccessful / RollbackFailed), the old instance ids are gone and each surviving guest, asked through SSM Run Command, reports its own instance id from IMDSv2 and the generation marker (/etc/sim2real-ltgen) that only the new version's user data writes. Recorded behavior only for the timings: the check polls terminal states and in-guest reports rather than fixed delays.

Emulator: 'Check timed out after 180.0s' — the flow never reaches a terminal refresh state with replaced, booted guests that answer a Run Command, so none of the id/generation assertions is evaluated. Hypothesis: the refresh is reported without actually terminating and relaunching the guests, and/or the relaunched guests never run their user data or never become Run Command targets. Open issues 20261108T203125Z-autoscaling-ec2-elbv2-startinstancerefresh-reports-successful-without-ever-terminating-the-instances.md and 20261107T075601Z-ec2-createvpc-from-an-ipam-pool-is-a.md cover neighbouring symptoms but neither carries this check's replay key.

Why: a client that rolls a new launch-template version out through an instance refresh cannot observe the rollout at all: nothing tells it which generation each running guest is on, and a refresh that never settles makes the whole replace-every-guest workflow unusable.

Fix: the instance-refresh engine (terminate/replace loop and terminal-state bookkeeping) and the guest-launch path it drives — user-data execution for the new launch-template version and the instance's availability as a Run Command target.

Cause 2 of 2 (check: ec2.op-contract-describe-reserved-instances-offerings): DescribeReservedInstancesOfferings serves a synthesized catalog that still offers instance families AWS has withdrawn from direct Reserved Instance purchase.

AWS: the same filtered query — Filters instance-type, OfferingClass 'standard', ProductDescription 'Linux/UNIX' — returns offerings for a1.large but an EMPTY ReservedInstancesOfferings list for m5.large, because that direct-purchase catalog entry is withdrawn. Recorded behavior only: the model constrains the shape, not the catalog contents, and the check records AWS's withdrawal. The check is tagged 'sweep:never' because any emulator that serves the catalog fails it, so treat this cause as the low-priority half of this issue.

Emulator: the m5.large query returns 18 offerings — 'AssertionError: expected AWS''s direct-purchase RI catalog for m5.large to be withdrawn (empty) ... returned 18 m5.large offering(s)'. The count is a per-run artifact of the generator; the requirement is that the withdrawn family yields an empty list under that filter while a still-offered family (a1.large) yields offerings whose InstanceType, OfferingClass and ProductDescription match the filters and whose Duration is 31536000 or 94608000 seconds.

Why: a client that enumerates purchasable Reserved Instances is shown offerings that do not exist on AWS, so cost-planning and purchase flows built on the emulator diverge from the real catalog.

Fix: the DescribeReservedInstancesOfferings catalog/state that backs the offering list — the set of (instance type, offering class, product description) combinations it publishes.

Traces: ec2.dasgrefresh-instance-refresh-replaces-every-guest-with-the-new-launch-template-version, ec2.op-contract-describe-reserved-instances-offerings.

## Evidence
Cause 1 — expected (real AWS): after creating launch-template generation 2 and calling StartInstanceRefresh(MinHealthyPercentage=100) on a desired-capacity-2 group, DescribeInstanceRefreshes reaches a terminal status, the two pre-refresh instance ids are no longer InService, and each new guest's Run Command output matches 'iid=<its own instance id> gen=<generation 2 marker>'.
Actual (emulator): ec2.dasgrefresh-instance-refresh-replaces-every-guest-with-the-new-launch-template-version -> 'Error: Check timed out after 180.0s' with no assertion reached.

Cause 2 — expected (real AWS): describe_reserved_instances_offerings(Filters=[{'Name':'instance-type','Values':['m5.large']}], OfferingClass='standard', ProductDescription='Linux/UNIX')['ReservedInstancesOfferings'] == [] , while the identical query for a1.large returns a non-empty list of matching offerings.
Actual (emulator): the m5.large query returns 18 offerings — 'AssertionError: expected AWS''s direct-purchase RI catalog for m5.large to be withdrawn (empty) — the same query that finds a1.large offerings above returned 18 m5.large offering(s), which means the withdrawal this check pins has reversed'. (18 is this run's value; the requirement is an empty list for the withdrawn family.)

## Steps to Reproduce
Cause 1:
1. Create a VPC/subnet, a launch template whose user data writes a generation marker to /etc/sim2real-ltgen, and an Auto Scaling group with desired capacity 2 on version 1.
2. Publish launch-template version 2 with a new marker and point the group at it.
3. Call StartInstanceRefresh with MinHealthyPercentage 100 and poll DescribeInstanceRefreshes for a terminal status; then SSM Run Command each surviving guest for 'iid=... gen=...'.
Observed: the flow never settles into a terminal refresh with replaced, Run-Command-answerable guests; the check exhausts its 180 s budget with no verdict.

Cause 2:
1. Call DescribeReservedInstancesOfferings with Filters instance-type=a1.large, OfferingClass=standard, ProductDescription=Linux/UNIX — note offerings are returned (correct).
2. Repeat with instance-type=m5.large.
Observed: a non-empty offering list (18 entries) where AWS returns an empty ReservedInstancesOfferings list.
