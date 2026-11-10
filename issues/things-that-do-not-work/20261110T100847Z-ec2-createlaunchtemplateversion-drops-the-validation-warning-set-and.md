Issue-Source: trace-sweep-divergence
Issue-Trace: asglf-activity-instance-profile-absent
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asglf-activity-instance-profile-absent autoscaling-asgssh-traj-template-key-name-fixes-and-group-refusals
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|CreateLaunchTemplateVersion|2xx->2xx|$.CreateLaunchTemplateVersionResponse.warning
Issue-Key: ec2|CreateLaunchTemplateVersion|2xx->2xx|$.CreateLaunchTemplateVersionResponse.launchTemplateVersion.launchTemplateData.keyName
Issue-Related: 20261108T223525Z-ec2-launch-template-version-reads-misattribute-the-creator.md
Issue-Commit: 8f521f25a30f45aed4bf452c3a7727cefd2e1709

# ec2 CreateLaunchTemplateVersion drops the validation warning set and the template's keyName

> _Note: this issue was identified against the code base at commit `8f521f25a30f45aed4bf452c3a7727cefd2e1709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplateVersion

## Description
Two net-new root causes in ec2 CreateLaunchTemplateVersion: the response's advisory warning set is never populated, and keyName supplied in LaunchTemplateData is not stored/echoed. Both are 200-vs-200 body divergences, so a client sees a successful create whose body is missing information AWS returns. This bundle carries 2 causes and both must be closed.

Cause 1 of 2: the Warning/errorSet advisory is never emitted for parameters AWS cannot validate.
AWS: Smithy (com.amazonaws.ec2#CreateLaunchTemplateVersion) output has member Warning -> ValidationWarning, xmlName 'warning', documented 'The error codes and error messages that are returned for the parameters or parameter combinations that are not valid when a new launch template or new version of a launch template is created.' ValidationWarning.Errors (xmlName 'errorSet') is a list of ValidationError{code (xmlName 'code'), message (xmlName 'message')}. CreateLaunchTemplateVersion still answers HTTP 200 and creates the version; a LaunchTemplateData naming an IAM instance profile that does not exist is reported as a warning item with code 'InvalidIamInstanceProfile.NotFound' and the message "The IamInstanceProfile.Name '<profile-name>' does not exist." — fixed text 'The IamInstanceProfile.Name ' + "'" + ' does not exist.' with the profile name taken verbatim from the request.
Emulator: HTTP 200 with $.CreateLaunchTemplateVersionResponse.warning absent entirely — no errorSet, no item, no code.
Why: the warning set is the only signal AWS gives that an otherwise-accepted launch template version will fail at launch time. A client (or an Auto Scaling group validating a template before use) sees a clean create and only discovers the broken instance profile when launches fail, which is exactly the diagnostic AWS provides up front.
Fix: the CreateLaunchTemplateVersion handler/serializer — the per-parameter advisory validation that builds ValidationWarning.Errors, and the serializer binding for the 'warning'/'errorSet'/'item' elements.
Traces: asglf-activity-instance-profile-absent#task#8. The same cluster also reports launchTemplateVersion.createdBy as 'arn:aws:iam::000000000000:root' where the recording has 'arn:aws:iam::000000000000:user/emulator'; that createdBy misattribution is already owned by open issue 20261108T223525Z-ec2-launch-template-version-reads-misattribute-the-creator.md and is NOT part of this bundle — the net-new cause here is the missing warning set.

Cause 2 of 2: keyName in LaunchTemplateData is dropped — the created version reads back with an empty key name.
AWS: Smithy — CreateLaunchTemplateVersion input LaunchTemplateData is RequestLaunchTemplateData (required) and the response's LaunchTemplateVersion.LaunchTemplateData is ResponseLaunchTemplateData; the response describes the version that was created, so every member supplied in the request (here KeyName, xmlName 'keyName') comes back with the value that was sent. There is no defaulting or blanking of KeyName.
Emulator: HTTP 200 with $.CreateLaunchTemplateVersionResponse.launchTemplateVersion.launchTemplateData.keyName = '' where the request supplied a key-pair name, so the new version silently carries no key pair.
Why: the client believes it fixed the template's key pair and the version it reads back disagrees; anything launched from that version has no key pair, and a read-after-write comparison of the template data fails. The loss is silent — status is 200 and no warning is emitted.
Fix: the CreateLaunchTemplateVersion handler's launch-template-data state (the request's KeyName must be stored on the new version) and the response projection of ResponseLaunchTemplateData.keyName.
Traces: autoscaling-asgssh-traj-template-key-name-fixes-and-group-refusals#task#18 (createdBy on that same step is '<shaped-not-asserted>' in the recording and is not a requirement here).

## Evidence
Replay of trace set simparity-wire-aws-v1 (89 fixtures, 1044 steps, 647 passed, score 62.0%). Two unowned mismatch clusters, both expected 200 / actual 200:

1) asglf-activity-instance-profile-absent#task#8 — ec2 CreateLaunchTemplateVersion: $.CreateLaunchTemplateVersionResponse.warning missing — expected {'errorSet': {'item': {'code': 'InvalidIamInstanceProfile.NotFound', 'message': "The IamInstanceProfile.Name 'sim2real-probe-asglf-prof-0b93e5a6-absent-profile' does not exist."}}}, got None. (The same cluster also reports createdBy 'arn:aws:iam::000000000000:root' vs expected 'arn:aws:iam::000000000000:user/emulator' — already owned by 20261108T223525Z-ec2-launch-template-version-reads-misattribute-the-creator.md.)

2) autoscaling-asgssh-traj-template-key-name-fixes-and-group-refusals#task#18 — ec2 CreateLaunchTemplateVersion: $.CreateLaunchTemplateVersionResponse.launchTemplateVersion.launchTemplateData.keyName value_mismatch — expected 'sim2real-probe-asgssh-key-7b3d-kp', got ''.

Model evidence: Smithy com.amazonaws.ec2#CreateLaunchTemplateVersion output = {LaunchTemplateVersion (xmlName 'launchTemplateVersion'), Warning -> ValidationWarning (xmlName 'warning')}; ValidationWarning.Errors -> ErrorSet (xmlName 'errorSet'); ValidationError = {Code (xmlName 'code'), Message (xmlName 'message')}; LaunchTemplateVersion.LaunchTemplateData -> ResponseLaunchTemplateData. The operation models no errors, i.e. these conditions are advisory on a 200 response.

## Steps to Reproduce
Replay the bound wire corpus against the emulator (replay_wire_corpus) and inspect these steps:
1. asglf-activity-instance-profile-absent step task#8 — CreateLaunchTemplateVersion whose LaunchTemplateData.IamInstanceProfile.Name names a profile that does not exist; the call answers 200, then check $.CreateLaunchTemplateVersionResponse.warning for an errorSet item with code 'InvalidIamInstanceProfile.NotFound'.
2. autoscaling-asgssh-traj-template-key-name-fixes-and-group-refusals step task#18 — CreateLaunchTemplateVersion whose LaunchTemplateData sets KeyName to an existing key pair; check $.CreateLaunchTemplateVersionResponse.launchTemplateVersion.launchTemplateData.keyName in the 200 body (and on a subsequent DescribeLaunchTemplateVersions of that version).
