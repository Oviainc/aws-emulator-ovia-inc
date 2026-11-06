Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter
Issue-Key: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc|run#14|KeyError|assert sorted(vs["AvailabilityZones"]) == sorted(p["az"] for p in pair), (
Issue-Key: ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter|run#11|AssertionError|assert msg == want, f"the refusal should explain the boundary exactly: expected {want!r}, got {msg!r}"
Issue-Commit: 09448fb928879ffb83c121d01adc49e65396319c

# ds: 2 net-new causes — DescribeDirectories' VpcSettings omits AvailabilityZones, and CreateMicrosoftAD's single-subnet refusal sentence is emulator-authored

> _Note: this issue was identified against the code base at commit `09448fb928879ffb83c121d01adc49e65396319c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDirectories, CreateMicrosoftAD

## Description
Two independent root causes in the Directory Service (ds) Microsoft AD control plane, both reached through ds__ec2 behavioral checks. Neither is covered by an open issue: the suite marked neither check ALREADY FILED nor OWNED. The open issues 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md (controller security group / per-subnet ENIs / DNS IPs) and 20261102T102235Z-ds-ec2-seven-net-new-root-causes-createmicrosoftad.md (other CreateMicrosoftAD validation causes) are adjacent but own neither of the two checks below.

Cause 1 of 2: DescribeDirectories reports a directory's VpcSettings without the AvailabilityZones member.
AWS: DescribeDirectories returns DirectoryDescription.VpcSettings as a DirectoryVpcSettingsDescription. Smithy: com.amazonaws.directoryservice#DirectoryVpcSettingsDescription has members VpcId, SubnetIds, SecurityGroupId and AvailabilityZones (documented "The list of Availability Zones that the directory is in."). For a Microsoft AD created with VpcSettings naming two subnets in two different Availability Zones, AvailabilityZones is present and is exactly the set of Availability Zones of those SubnetIds — a value DERIVED from the request's subnets, never an invented or fixed zone list.
Emulator: the VpcSettings object it serializes carries VpcId and SubnetIds (the check asserts both and they match) but contains NO AvailabilityZones key at all, so reading it raises KeyError: 'AvailabilityZones'. Everything asserted before it (Name, ShortName, Description, Type=MicrosoftAD, Edition=Standard, Size=Small, VpcId, SubnetIds) was already correct, so this is purely a missing output member, not a wrong value.
Why: a client that reads VpcSettings.AvailabilityZones to learn which zones the directory's controllers occupy — to place a dependent resource in a matching zone, to assert AZ spread, or to render the directory — gets a missing key instead of the two zones, and any SDK/consumer treating the member as present fails outright. The zone information a two-AZ managed AD is defined by is unobservable.
Fix: the DescribeDirectories read path and the directory record behind it (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/describe_directories.rs and the directory's stored VPC settings in that crate's state): derive the Availability Zones from the stored SubnetIds when the directory is created and serialize them into DirectoryVpcSettingsDescription.AvailabilityZones. Only the AvailabilityZones member is evidenced as absent by this check.
Traces: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc.

Cause 2 of 2: CreateMicrosoftAD's single-subnet refusal carries an emulator-authored sentence.
AWS: CreateMicrosoftAD with VpcSettings.SubnetIds holding only one subnet is refused with 400 InvalidParameterException (Smithy: com.amazonaws.directoryservice#InvalidParameterException, client error, HTTP 400, members Message and RequestId). Its message's FIXED text is "Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. ", followed by ": RequestId: <request id>"; the request id is the only variable part and is the id of that call (per-run, not part of the requirement).
Emulator: the status and the code are right (InvalidParameterException), but the sentence is re-worded: "Invalid subnet ID(s). The two subnets must be in different Availability Zones. : RequestId: <request id>". The fixed text "They must correspond to two subnets in different Availability Zones." is replaced with "The two subnets must be in different Availability Zones.".
Why: a client that matches, logs or surfaces AWS's refusal sentence — and any test or runbook keyed on it — does not recognise the refusal, even though the error code is right; the emulator's sentence also mis-describes the boundary (it says the two subnets must be in different zones, when the request supplied only one subnet and the real rule is that two subnets in different zones are required).
Fix: the CreateMicrosoftAD request validation's refusal message template (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_microsoft_ad.rs) — emit AWS's sentence for the subnet-count/zone boundary rather than emulator prose.
Traces: ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter.

## Evidence
Cause 1 — ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc [error]:
  File "check_ds__ec2_create_microsoft_ad_round_trips_name_short_name_description_edition_and_vpc.py", line 33, in run
    assert sorted(vs["AvailabilityZones"]) == sorted(p["az"] for p in pair)
  KeyError: 'AvailabilityZones'
  Expected: d["VpcSettings"]["AvailabilityZones"] present and equal (as a set) to the Availability Zones of the two SubnetIds the create supplied.
  Actual: the VpcSettings mapping has no AvailabilityZones key (VpcId and SubnetIds asserted equal first and passed).
  Smithy: com.amazonaws.directoryservice#DirectoryVpcSettingsDescription members = VpcId, SubnetIds, SecurityGroupId, AvailabilityZones ("The list of Availability Zones that the directory is in.").

Cause 2 — ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter [mismatch]:
  Code: InvalidParameterException (expected and observed — correct).
  Expected message: "Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. : RequestId: <request id>"
  Actual message:   "Invalid subnet ID(s). The two subnets must be in different Availability Zones. : RequestId: <request id>"
  (the request id in the recorded run was 00000000-0000-4000-8000-000000000001; the check reads it back from the response, so only the sentence diverges)
  AssertionError: the refusal should explain the boundary exactly.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite); the two checks below are listed under '### ds__ec2'.
2. Cause 1: create a VPC with two /24 subnets in two different Availability Zones; call ds CreateMicrosoftAD(Name=<x>.test, Password=..., Edition=Standard, ShortName=..., Description=..., VpcSettings={VpcId, SubnetIds=[both subnets]}); then call ds DescribeDirectories and read DirectoryDescriptions[0].VpcSettings — VpcId and SubnetIds are present and correct, AvailabilityZones is absent (AWS returns the two subnets' zones).
3. Cause 2: with the same VPC, call ds CreateMicrosoftAD with VpcSettings={VpcId, SubnetIds=[one subnet]}; inspect the ClientError — the code is InvalidParameterException but the message sentence is "Invalid subnet ID(s). The two subnets must be in different Availability Zones." instead of AWS's "Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones.".
