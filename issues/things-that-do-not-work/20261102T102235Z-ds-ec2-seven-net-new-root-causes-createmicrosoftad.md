Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.create-microsoft-ad-omitted-short-name-defaults-to-first-label-cut-to-15-and-description-stays-absent ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc ds__ec2.create-microsoft-ad-short-name-longer-than-15-characters-rejected-invalid-parameter ds__ec2.create-microsoft-ad-with-a-subnet-of-another-vpc-rejected-client-exception ds__ec2.create-microsoft-ad-with-an-unknown-vpc-id-rejected-client-exception ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable
Issue-Key: ds__ec2.create-microsoft-ad-omitted-short-name-defaults-to-first-label-cut-to-15-and-description-stays-absent|run#7|AssertionError|assert d.get("ShortName") == first_label[:15], (
Issue-Key: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc|run#10|AssertionError|assert d.get("Size") == "Small", f"a Standard Microsoft AD reports Size {d.get('Size')!r}"
Issue-Key: ds__ec2.create-microsoft-ad-short-name-longer-than-15-characters-rejected-invalid-parameter|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ds__ec2.create-microsoft-ad-with-a-subnet-of-another-vpc-rejected-client-exception|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ds__ec2.create-microsoft-ad-with-an-unknown-vpc-id-rejected-client-exception|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable|run#5|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Commit: 855e46d3b98757a511fcacb47a7c9e35083e2051

# ds__ec2: seven net-new root causes — CreateMicrosoftAD accepts three requests AWS refuses, ShortName/Size are wrong on reads, and the directory is instantly Active with no EC2 network resources

> _Note: this issue was identified against the code base at commit `855e46d3b98757a511fcacb47a7c9e35083e2051`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
Seven net-new behavioral root causes on the Directory Service / EC2 boundary. Three requests AWS refuses are accepted and create a directory; two read members are wrong or missing; the directory never provisions the EC2 resources it must; and it never passes through the Creating stage. A fixer must close ALL SEVEN before submitting.

Cause 1 of 7 — an omitted ShortName is defaulted to the whole first label, upper-cased, instead of the first label truncated to 15 characters with its case preserved.
AWS: Smithy: CreateMicrosoftAD.ShortName is optional (DirectoryShortName, @pattern ^[^\\\\/:*?"<>|.]+[^\\\\/:*?"<>|]*$) and is reported back as DirectoryDescription.ShortName. Recorded behavior: when ShortName is omitted, the default is the Name's FIRST DNS label truncated to the NetBIOS limit of 15 characters, with the label's case preserved. Variable part: the label is taken from the request's Name; the 15-character limit is the fixed rule.
Emulator: returns the entire first label, upper-cased — e.g. for a 22-character label it answers 'SIM2REALDSAB4241F696BB' where AWS answers 'sim2realdsab424'.
Why: ShortName is the directory's NetBIOS name; a caller that joins machines or builds OU paths from it uses a value longer than NetBIOS allows and in the wrong case, so nothing it derives matches AWS.
Fix: CreateMicrosoftAD's handler, the ShortName defaulting step (truncate the first label to 15 chars, do not case-fold), and the DescribeDirectories projection that reports it.
Traces: ds__ec2.create-microsoft-ad-omitted-short-name-defaults-to-first-label-cut-to-15-and-description-stays-absent.

Cause 2 of 7 — DescribeDirectories omits DirectoryDescription.Size for a Managed Microsoft AD.
AWS: Smithy: DirectoryDescription.Size is the DirectorySize enum (Small | Large). Recorded behavior: a Standard-edition Managed Microsoft AD reports Size "Small".
Emulator: the member is absent (reads back as None) while Name, ShortName, Description, Type, Edition and VpcSettings on the same response are right.
Why: callers that size cost or capacity off Size get a null where AWS always sends an enum value.
Fix: the DescribeDirectories response projection — derive Size from the directory's edition/type in state.
Traces: ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc. (Open issue 20261029T200700Z-ds-7-net-new-root-causes-connectdirectory-describesettings.md mentions DescribeDirectories omitting Size; it carries no behavioral check id and does not own this check.)

Cause 3 of 7 — a ShortName longer than 15 characters is accepted.
AWS: Smithy: CreateMicrosoftAD models InvalidParameterException (client, HTTP 400) — "One or more parameters are not valid." Recorded behavior: a 16-character ShortName (over the NetBIOS limit of 15) is refused and no directory is created. Message template — fixed text, with no variable parts beyond the envelope: " Invalid directory short name. The short name must be between 1 and 15 characters, and can consist of alphanumeric characters and some special characters (! @ # $ % ^ & ( ) - _ ' { } or ~). The following special characters are not allowed: ^ \\ / : * ? \" < > . and |. The name cannot begin with a period." then " : RequestId: " <request id> (note the leading space before "Invalid").
Emulator: the call SUCCEEDS (HTTP 200) and a directory is created.
Why: a caller gets a Managed Microsoft AD whose NetBIOS name AWS would never accept, and its input-validation test passes against the emulator and fails against AWS.
Fix: CreateMicrosoftAD's ShortName validation, before any state is written.
Traces: ds__ec2.create-microsoft-ad-short-name-longer-than-15-characters-rejected-invalid-parameter.

Cause 4 of 7 — VpcSettings naming VPC A with a subnet belonging to VPC B is accepted.
AWS: Smithy: CreateMicrosoftAD.VpcSettings is @required (DirectoryVpcSettings: VpcId + SubnetIds) and the operation models ClientException (client, HTTP 400). Recorded behavior: the service resolves the subnets against EC2 and refuses when they do not all belong to the named VPC; no directory is created. Message template — fixed text, no variable parts: "Invalid subnet ID(s). Both subnets must belong to the specified VPC." then " : RequestId: " <request id>.
Emulator: the call SUCCEEDS (HTTP 200) even though EC2 reports the two subnets in two different VPCs.
Why: the directory is placed in a network topology AWS rejects, so every downstream assumption about its VPC (routing, security groups, DNS) is unsound, and the caller never learns its VpcSettings were wrong.
Fix: CreateMicrosoftAD's VpcSettings validation — resolve each SubnetId through EC2 and require VpcId ownership before accepting.
Traces: ds__ec2.create-microsoft-ad-with-a-subnet-of-another-vpc-rejected-client-exception.

Cause 5 of 7 — a VpcId EC2 does not know is accepted.
AWS: Smithy: same VpcSettings member and ClientException as cause 4. Recorded behavior: a VpcId naming no VPC is refused with HTTP 400 ClientException and no directory is created. Message template — fixed text, no variable parts: "Invalid VPC ID." then " : RequestId: " <request id>.
Emulator: the call SUCCEEDS (HTTP 200) with real subnets under a fabricated VpcId.
Why: a typo'd or stale VPC id silently produces a directory bound to nothing, instead of the immediate 400 a caller's provisioning code is written against.
Fix: CreateMicrosoftAD's VpcSettings validation — resolve VpcId through EC2 before accepting. (Causes 4 and 5 are the same missing cross-service resolution but two distinct refusals with two distinct messages.)
Traces: ds__ec2.create-microsoft-ad-with-an-unknown-vpc-id-rejected-client-exception.

Cause 6 of 7 — a Managed Microsoft AD never provisions its controllers security group or its ENIs in the caller's VPC, so the delete path has nothing to release.
AWS: recorded behavior: after CreateMicrosoftAD the directory reports DirectoryDescription.VpcSettings.SecurityGroupId, EC2 lists that group in the caller's VPC, and EC2 lists one directory ENI per chosen subnet whose Description names the directory. DeleteDirectory then releases them: DescribeNetworkInterfaces on each released ENI answers InvalidNetworkInterfaceID.NotFound ("The networkInterface ID '<eni id>' does not exist") and DescribeSecurityGroups on the group answers InvalidGroup.NotFound ("The security group '<group id>' does not exist") — the ids being state values, the codes and sentence fixed text.
Emulator: VpcSettings.SecurityGroupId never appears, so the check cannot get past the first poll and times out; no ENIs are ever created either, so the whole create→delete→release contract is unobservable.
Why: a caller cannot delete its own subnets/VPC in the right order, cannot attach rules to the controllers group, and cannot verify cleanup — the EC2-visible footprint of a directory simply does not exist.
Fix: CreateMicrosoftAD's provisioning path (mint the controllers security group and one ENI per placement subnet in the caller's VPC through the EC2 state) and DeleteDirectory's teardown that releases them.
Traces: ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc. (Open issue 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md describes this footprint for other checks; it does not carry this check id.)

Cause 7 of 7 — a new Microsoft AD is instantly Active, so the Creating-stage refusal of UpdateNumberOfDomainControllers is unreachable.
AWS: Smithy: UpdateNumberOfDomainControllers takes DirectoryId and DesiredNumber (@range min 2) and models InvalidParameterException and DirectoryUnavailableException (client, HTTP 400). Recorded behavior: a freshly created directory passes through DirectoryStage "Creating" before "Active", and scaling it while Creating is refused InvalidParameterException with — fixed text, no variable parts — "The directory is currently unavailable for updates. The directory status must be Active when making changes. Please try again in a few minutes." then " : RequestId: " <request id>; the refused call leaves DesiredNumberOfDomainControllers and Stage unchanged.
Emulator: immediately after CreateMicrosoftAD the directory already reports Stage 'Active', so the Creating state never exists and this refusal can never be produced.
Why: every caller that polls for Active sees it instantly and every stage-gated refusal in the service becomes unreachable, so clients that must wait before scaling are never told to wait.
Fix: the directory lifecycle in the ds state machine — a created directory must enter Creating and advance to Active on the clock, and the stage-gated refusals (here UpdateNumberOfDomainControllers) must consult it.
Traces: ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable. (Open issue 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly.md describes the instant-Active defect for other checks; it does not carry this check id.)

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout (ds__ec2: 3/21 passed). RequestIds and generated ids/names are per-run and are not requirements.

1) ds__ec2.create-microsoft-ad-omitted-short-name-... — CreateMicrosoftAD with ShortName omitted and a 22-character first label.
   expected ShortName: the first label cut to 15 chars, case preserved (e.g. 'sim2realdsab424')
   actual:   'SIM2REALDSAB4241F696BB'

2) ds__ec2.create-microsoft-ad-round-trips-name-short-name-description-edition-and-vpc — DescribeDirectories on the created directory.
   expected: Size == "Small"; actual: Size is absent (None). Name/ShortName/Description/Type/Edition asserted before it all matched.

3) ds__ec2.create-microsoft-ad-short-name-longer-than-15-characters-rejected-invalid-parameter — CreateMicrosoftAD(ShortName="S2RABCDEFGHIJKLM", real VpcSettings).
   expected: HTTP 400 InvalidParameterException with " Invalid directory short name. The short name must be between 1 and 15 characters, and can consist of alphanumeric characters and some special characters (! @ # $ % ^ & ( ) - _ ' { } or ~). The following special characters are not allowed: ^ \\ / : * ? \" < > . and |. The name cannot begin with a period. : RequestId: <rid>"
   actual:   the call succeeded (harness: "the call succeeded; real AWS rejects it, so there is no error code").

4) ds__ec2.create-microsoft-ad-with-a-subnet-of-another-vpc-rejected-client-exception — EC2 control confirms the two subnets sit in two different VPCs and different AZs; CreateMicrosoftAD(VpcSettings={VpcId: A, SubnetIds:[subnet in A, subnet in B]}).
   expected: HTTP 400 ClientException "Invalid subnet ID(s). Both subnets must belong to the specified VPC. : RequestId: <rid>"
   actual:   the call succeeded.

5) ds__ec2.create-microsoft-ad-with-an-unknown-vpc-id-rejected-client-exception — CreateMicrosoftAD(VpcSettings={VpcId: <fabricated vpc-0...>, SubnetIds:[two real subnets in different AZs]}).
   expected: HTTP 400 ClientException "Invalid VPC ID. : RequestId: <rid>"
   actual:   the call succeeded.

6) ds__ec2.delete-directory-releases-its-controllers-security-group-and-enis-from-the-vpc — "Check timed out after 60.0s" while polling DescribeDirectories for VpcSettings.SecurityGroupId, which never appears; the ENI assertions and the post-delete InvalidNetworkInterfaceID.NotFound / InvalidGroup.NotFound expectations are therefore never reached.

7) ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable — immediately after CreateMicrosoftAD.
   expected Stage: 'Creating' (then InvalidParameterException "The directory is currently unavailable for updates. The directory status must be Active when making changes. Please try again in a few minutes. : RequestId: <rid>" for UpdateNumberOfDomainControllers(DesiredNumber=3))
   actual:   Stage is already 'Active' — "precondition: the directory should be Creating, is 'Active'".

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): CreateMicrosoftAD.ShortName optional with @pattern ^[^\\\\/:*?"<>|.]+[^\\\\/:*?"<>|]*$, VpcSettings @required, errors ClientException/DirectoryLimitExceededException/InvalidParameterException/ServiceException/UnsupportedOperationException (client ones HTTP 400); UpdateNumberOfDomainControllers.DesiredNumber @range min 2 with InvalidParameterException and DirectoryUnavailableException modeled.

## Steps to Reproduce
1. Clone this checkout and replay suite simparity-behavioral-aws-v1 against the booted emulator (these checks drive both ds and ec2).
2. Cause 1: create a VPC with two subnets in different AZs, then CreateMicrosoftAD(Name=<label longer than 15 chars>.test, Password, Edition=Standard, VpcSettings) with ShortName OMITTED; read DescribeDirectories.ShortName.
3. Cause 2: same created directory — read DescribeDirectories.Size.
4. Cause 3: CreateMicrosoftAD with ShortName="S2RABCDEFGHIJKLM" (16 chars) and real VpcSettings; observe HTTP 200 instead of InvalidParameterException.
5. Cause 4: create two VPCs, pass VpcId of the first with one subnet from each; observe HTTP 200 instead of ClientException "Invalid subnet ID(s)...".
6. Cause 5: pass a fabricated VpcId with two real subnets; observe HTTP 200 instead of ClientException "Invalid VPC ID.".
7. Cause 6: create a directory and poll DescribeDirectories for VpcSettings.SecurityGroupId, then DescribeNetworkInterfaces filtered on the VPC for ENIs naming the directory; both stay empty, so DeleteDirectory's release cannot be observed.
8. Cause 7: immediately after CreateMicrosoftAD read DescribeDirectories.Stage (it is 'Active', never 'Creating'), then call UpdateNumberOfDomainControllers(DesiredNumber=3) and observe that the Creating-stage InvalidParameterException can never be produced.
