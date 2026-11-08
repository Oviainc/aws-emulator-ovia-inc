Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc
Issue-Key: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc|run#16|AssertionError|assert d["Stage"] == "Creating", f"a fresh directory left Requested for {d['Stage']!r}"
Issue-Commit: 32a26f18a0fb4dc356c9d3ce1ecec069a0aff304

# ds__ec2: a fresh Managed Microsoft AD leaves Requested straight for Active, so the Creating stage the happy-path read asserts is never observable

> _Note: this issue was identified against the code base at commit `32a26f18a0fb4dc356c9d3ce1ecec069a0aff304`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
One net-new cause, on the CreateMicrosoftAD/DescribeDirectories happy-path read.

Cause 1 of 1: a directory created with CreateMicrosoftAD is reported Active as soon as it leaves Requested, instead of spending the provisioning window in Creating.
AWS: Smithy — DescribeDirectories returns DirectoryDescriptions whose Stage is the DirectoryStage enum; the staged lifecycle itself is recorded behavior only. Recorded behavior: CreateMicrosoftAD answers with the directory in Stage 'Requested' for a few seconds, then 'Creating' for the whole domain-controller provisioning window (tens of minutes), and only afterwards 'Active'. The check polls past 'Requested' with a 180 s budget and asserts the first durable stage.
Emulator: the first stage observed after 'Requested' is already 'Active' — the directory is never in 'Creating'.
Why: the emulator reports a directory as ready to serve the instant it is created, so a client that waits for Active is never actually made to wait and, more importantly, every stage-gated behaviour becomes unreachable: nothing can be refused with ClientException / DirectoryUnavailableException ('The operation is not supported for directories in this state. Directory Status: Creating') because that state does not occur. The same read also asserts the rest of the round trip (Name, ShortName, Description, Type/Edition, Size, VpcSettings, AvailabilityZones, DesiredNumberOfDomainControllers), which it reaches, so Stage is the only divergence here.
Fix: the directory lifecycle/stage progression in the CreateMicrosoftAD handler and the stage the ds service state writes and advances for it — Requested must settle on Creating, and Creating on Active one span later, rather than Requested settling directly on Active.
Traces: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc. An open issue, 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md, describes the same instant-Active stage for five other ds__ec2 checks; it does not carry this check's replay key, so this check must be re-replayed to confirm the fix and is filed here. A fixer should close them together.

## Evidence
ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc: File "check_ds__ec2_create_microsoft_ad_round_trips_name_short_name_description_edition_and_vpc.py", line 38: `assert d["Stage"] == "Creating"` -> AssertionError: a fresh directory left Requested for 'Active'. `d` is DescribeDirectories(DirectoryIds=[<new id>]).DirectoryDescriptions[0], polled every 2 s for up to 180 s until Stage != 'Requested'. Expected Stage 'Creating'; actual 'Active'. The directory id, VPC/subnet ids, domain name and ShortName are per-run values and not requirements; the requirement is the stage sequence Requested -> Creating -> Active.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite(); replay ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc.
2. Minimal repro: ec2:CreateVpc + two ec2:CreateSubnet in two AZs; ds:CreateMicrosoftAD(Name='<unique>.test', ShortName='S2RXXXXXX', Description='managed AD with LDAPS probe', Password=..., Edition='Standard', VpcSettings={VpcId, SubnetIds:[a,b]}).
3. Poll ds:DescribeDirectories(DirectoryIds=[<id>]) every 2 s and read DirectoryDescriptions[0].Stage until it is not 'Requested' -> expect 'Creating'; observe 'Active'.
