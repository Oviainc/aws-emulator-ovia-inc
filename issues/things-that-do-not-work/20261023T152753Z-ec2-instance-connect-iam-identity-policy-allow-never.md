Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2-instance-connect.send-ssh-public-key-iam-deny-auth-exception-rejected ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected
Issue-Shape-Id: com.amazonaws.ec2instanceconnect#AWSEC2InstanceConnectService
Issue-Key: ec2-instance-connect.send-ssh-public-key-iam-deny-auth-exception-rejected|_poll_until_code#15|Timeout|time.sleep(interval_s)
Issue-Key: ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected|_latest_al2_ami#4|AssertionError|raise AssertionError("no Amazon Linux 2 AMI found in this region")
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 Instance Connect: IAM identity-policy Allow never reaches SendSSHPublicKey, and DescribeImages exposes no Amazon-owned AMIs

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendSSHPublicKey

## Description
Two independent defects block the EC2 Instance Connect authorization surface. BOTH must be closed before submitting.

Cause 1 of 2: a freshly created IAM user whose identity policy explicitly Allows ec2-instance-connect:SendSSHPublicKey on Resource '*' never gets past the authorization layer, so SendSSHPublicKey never reaches resource lookup.
AWS: an identity holding an Allow for ec2-instance-connect:SendSSHPublicKey is authorized, and the call then proceeds to resource resolution; for a well-formed but nonexistent InstanceId it is refused with EC2InstanceNotFoundException. Smithy (model services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/smithy-model/com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice.json, sdkId 'EC2 Instance Connect', awsJson1_1, sigv4): EC2InstanceNotFoundException and AuthException are service-common errors bound to every operation including SendSSHPublicKey. The IAM-layer refusal code (AccessDeniedException) is not in the EIC model and is recorded behavior only. Credential propagation after iam:CreateAccessKey is tolerated by the check as a transient, so a short-lived InvalidClientTokenId/AuthFailure is NOT the defect; a permanent one is.
Emulator: the request never returns EC2InstanceNotFoundException. The check polls the identical call every 4s and only tolerates {AccessDeniedException, InvalidClientTokenId, AuthFailure, AccessDenied, UnrecognizedClientException} — any other code raises immediately — and it never raised an unexpected-code assertion, so the emulator kept answering with one of those authorization/credential refusals for the whole window until the 60s check budget expired. Either IAM-minted access keys are never honoured for this service, or authorization is refused by default and the identity-based Allow is not evaluated.
Why: a client that provisions a least-privilege identity for push-SSH-key workflows cannot make a single successful call, and no IAM semantics on this service can be exercised: explicit-Deny-overrides-Allow, condition evaluation and allow/deny transitions are all unreachable. Worse for fidelity work, a permanent blanket refusal is indistinguishable from correct denial, so an emulator in this state silently passes under-specified authorization checks.
Fix: sigv4 credential resolution for IAM-created access keys plus identity-policy evaluation on the authorization path in front of the SendSSHPublicKey handler (the EIC service crate's auth layer and the IAM policy evaluator it consults); the handler must then fall through to instance lookup and raise EC2InstanceNotFoundException for an absent instance.
Traces: ec2-instance-connect.send-ssh-public-key-iam-deny-auth-exception-rejected.

Cause 2 of 2: ec2:DescribeImages returns no Amazon-owned AMIs, so nothing can be launched and every instance-backed check is unreachable.
AWS: DescribeImages with Owners=['amazon'] and Filters name='amzn2-ami-hvm-*-x86_64-gp2', state='available' returns a non-empty Images list of public Amazon-owned AMIs in every commercial region; this catalog is region data, not account-specific state, and needs no prior setup. Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): DescribeImages input Owners (xmlName 'Owner'), Filters (xmlName 'Filter'), output Images (xmlName 'imagesSet'), paginated by NextToken; no modeled errors — the empty result is a data gap, not a rejection. Specific image ids and CreationDate values are environment-specific and are NOT the requirement; the requirement is that the Amazon-owned, state='available' AMI catalog is non-empty and matchable by the documented name/state filters.
Emulator: the call is answered 200 with an empty Images list, so the check's AMI selection fails with 'no Amazon Linux 2 AMI found in this region' before it can launch an instance.
Why: every tutorial-shaped and real-world flow starts by resolving a current Amazon Linux AMI and then calling RunInstances. With an empty catalog the client cannot pick an image at all, and the whole downstream surface (RunInstances, instance waiters, tag-conditioned authorization against a running instance) is unreachable. This also masks defects behind it rather than reporting them.
Fix: seed the Amazon-owned public AMI catalog in EC2 state and make the DescribeImages handler honour the Owners and name/state Filters against it.
Traces: ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected.

Notes on adjacent filings: open issue 20261023T102725Z-ec2-describe-reads-return-wrong-page-contents-no.md covers EC2 describe pagination/state and a missing default-subnet baseline, but says nothing about an empty Amazon-owned AMI catalog, so it does not cover cause 2. The other two ec2-instance-connect checks in this suite fail on unimplemented EC2 scaffolds (GetSerialConsoleAccessStatus, CreateInternetGateway) and are already filed separately.

## Evidence
Cause 1: check ec2-instance-connect.send-ssh-public-key-iam-deny-auth-exception-rejected, verdict mismatch, 'Check timed out after 60.0s'. Expected first beat: SendSSHPublicKey(InstanceId='i-<17 hex>', InstanceOSUser='ec2-user', SSHPublicKey=<format-valid ssh-rsa>) as an identity with only Allow ec2-instance-connect:SendSSHPublicKey -> ClientError code 'EC2InstanceNotFoundException' (recorded message example: 'Instance not found.'). Actual: never observed; the polling loop raises on any code outside {AccessDeniedException, InvalidClientTokenId, AuthFailure, AccessDenied, UnrecognizedClientException} and raised no such assertion, so the emulator returned one of those authorization/credential refusals continuously until the check budget expired. The later beats (explicit Deny -> AccessDeniedException, Deny deleted -> EC2InstanceNotFoundException) were therefore never reached.

Cause 2: check ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected, verdict mismatch. Traceback: line 27 'ami_id = _latest_al2_ami(ec2)' -> line 164 'raise AssertionError("no Amazon Linux 2 AMI found in this region")'. The call made was ec2:DescribeImages(Owners=['amazon'], Filters=[{'Name':'name','Values':['amzn2-ami-hvm-*-x86_64-gp2']},{'Name':'state','Values':['available']}]); expected a non-empty Images list, actual resp['Images'] == [].

## Steps to Reproduce
Cause 1: (a) iam:CreateUser; iam:PutUserPolicy with {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2-instance-connect:SendSSHPublicKey","Resource":"*"}]}; iam:CreateAccessKey. (b) With those credentials call ec2-instance-connect:SendSSHPublicKey(InstanceId='i-' + 17 hex chars that exist nowhere in the account, InstanceOSUser='ec2-user', SSHPublicKey=<format-valid ssh-rsa key>). (c) Expect ClientError code EC2InstanceNotFoundException within a few seconds of credential propagation; observe instead a persistent authorization/credential refusal code that never transitions. (d) Then add an explicit Deny on the same action and confirm the code cannot be distinguished from (c), which is the fidelity loss.

Cause 2: call ec2:DescribeImages(Owners=['amazon'], Filters=[{'Name':'name','Values':['amzn2-ami-hvm-*-x86_64-gp2']},{'Name':'state','Values':['available']}]) against a fresh emulator and inspect Images — it is empty, where AWS returns Amazon-owned available AMIs. Any flow that selects an AMI and then calls ec2:RunInstances stops here.

Replay the two check ids in trace_ids to confirm both causes are closed.
