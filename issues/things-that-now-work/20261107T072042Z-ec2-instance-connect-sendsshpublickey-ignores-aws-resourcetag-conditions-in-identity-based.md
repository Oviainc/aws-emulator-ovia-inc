Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected
Issue-Shape-Id: com.amazonaws.ec2instanceconnect#AWSEC2InstanceConnectService
Issue-Key: ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# SendSSHPublicKey ignores aws:ResourceTag conditions in identity-based policies and returns Success for an instance the policy does not cover

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendSSHPublicKey

## Description
Cause 1 of 1: the emulator does not evaluate resource-tag conditions (aws:ResourceTag/<key>) attached to an identity-based policy statement when authorizing ec2-instance-connect:SendSSHPublicKey, so a request that the policy does NOT cover is authorized and executed instead of being refused.

AWS: an IAM user whose only grant is `{"Effect":"Allow","Action":"ec2-instance-connect:SendSSHPublicKey","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/sim2real-team":"green"}}}` is authorized for SendSSHPublicKey only when the target EC2 instance carries the tag sim2real-team=green. Against an instance tagged sim2real-team=blue the condition does not match, no statement allows the action, and the call is refused with HTTP 403 and error code `AccessDeniedException`, message: `User: <caller-user-arn> is not authorized to perform: ec2-instance-connect:SendSSHPublicKey on resource: <instance-arn> because no identity-based policy allows the ec2-instance-connect:SendSSHPublicKey action`. In that message the fixed text is `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`; the variable parts are the caller's IAM user ARN (from the signing credentials), the resource ARN `arn:<partition>:ec2:<region>:<account>:instance/<instance-id>` built from the request's InstanceId plus the request's region/account/partition, and the action name. After retagging the SAME instance to sim2real-team=green, the identical call must be authorized and return 200 with Success=true. Smithy (services/service-com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice/smithy-model/com-amazonaws-ec2instanceconnect-awsec2instanceconnectservice.json, sdkId 'EC2 Instance Connect', awsJson1_1, sigv4): SendSSHPublicKey input {InstanceId required pattern ^i-[a-f0-9]+$, InstanceOSUser required, SSHPublicKey required @length 80..4096}, output {RequestId, Success:boolean default false}; its modeled errors are AuthException(403), EC2InstanceNotFoundException(404), EC2InstanceStateInvalidException(400), EC2InstanceUnavailableException(503), InvalidArgsException(400), ServiceException(500), ThrottlingException(429). The IAM-layer refusal code AccessDeniedException is not in the EIC model, so its code/status/message above are recorded behavior only.

Emulator: the call is ACCEPTED. The restricted identity's first SendSSHPublicKey against the blue-tagged instance returns HTTP 200 with body `{"RequestId": "<uuid>", "Success": true}` (content-type application/x-amz-json-1.1), where AWS returns 403 AccessDeniedException. The check asserts a rejection and fails with `AssertionError: expected the call to be rejected, but it succeeded and returned {'RequestId': ..., 'Success': True, ...}`. No per-run value (the request id, the generated instance/user names) is part of the requirement — the divergence is accept-vs-refuse.

Why: a client that scopes push-SSH-key permission to a tag (the documented way to fence EIC access to one team's or one environment's instances) gets no fencing at all: any instance in the account is reachable by any identity that holds the tag-conditioned Allow. This is an authorization bypass, and it is the dangerous direction — code and tests that depend on the deny are silently green on the emulator and fail only in production, and conversely a real tag-condition regression cannot be detected here. It also makes the whole condition-evaluation surface (StringEquals on aws:ResourceTag, and the allow-after-retag transition) untestable.

Fix: the IAM policy evaluator consulted by the EIC authorization layer in front of the SendSSHPublicKey handler — condition-block evaluation must resolve aws:ResourceTag/<key> from the target instance's tags in EC2 state for the InstanceId in the request, treat a non-matching condition as a non-matching statement, and when no statement matches emit the AccessDeniedException refusal (403) with the message template above, built from the caller ARN, the instance ARN and the action.

Related open issue 20261023T152753Z-ec2-instance-connect-iam-identity-policy-allow-never.md listed this check id, but it was grounded in a different failure (its cause 2, ec2:DescribeImages returning an empty Amazon-owned AMI catalog, which this replay now gets past) and its cause 1 is the opposite failure mode (an unconditional Allow never being honoured). Neither describes a tag condition being ignored, so this cause is not covered there.

Traces: ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected.

## Evidence
Check ec2-instance-connect.send-ssh-public-key-resource-tag-condition-mismatch-auth-exception-rejected, verdict mismatch.

Setup performed by the check: ec2:CreateVpc + ec2:CreateSubnet; ec2:DescribeImages(Owners=['amazon'], Filters name='amzn2-ami-hvm-*-x86_64-gp2', state='available'); ec2:RunInstances with instance tags {Name: <generated>, sim2real-team: 'blue'}; waiter instance_running; iam:CreateUser + iam:PutUserPolicy with Allow ec2-instance-connect:SendSSHPublicKey on Resource '*' Condition StringEquals aws:ResourceTag/sim2real-team = 'green'; iam:CreateAccessKey.

Call under test, signed with those restricted credentials: ec2-instance-connect:SendSSHPublicKey(InstanceId=<the blue-tagged instance>, InstanceOSUser='ec2-user', SSHPublicKey=<format-valid ssh-rsa, 2048-bit wire blob>).

Expected: ClientError, HTTP 403, Error.Code 'AccessDeniedException', Error.Message 'User: <user-arn> is not authorized to perform: ec2-instance-connect:SendSSHPublicKey on resource: arn:<partition>:ec2:<region>:<account>:instance/<instance-id> because no identity-based policy allows the ec2-instance-connect:SendSSHPublicKey action'.

Actual: HTTP 200, content-type application/x-amz-json-1.1, content-length 67, body {'RequestId': '983692ea-84ac-88bf-e9c5-3c58e909047b', 'Success': True} — the request id is a per-run value and not part of the requirement. Failure surfaced as:
  File "check_ec2_instance_connect_send_ssh_public_key_resource_tag_condition_mismatch_auth_exception_rejected.py", line 94, in _poll_stable_error -> line 84, in _error
  AssertionError: expected the call to be rejected, but it succeeded and returned {'RequestId': ..., 'Success': True, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Because the first beat was accepted, the second beat (retag the same instance to sim2real-team=green and confirm the identical call now returns Success=true) was never reached, so whether the allow-side of the condition is evaluated is still unverified.

## Steps to Reproduce
1. ec2:CreateVpc / ec2:CreateSubnet, resolve an Amazon Linux 2 AMI via ec2:DescribeImages(Owners=['amazon']), then ec2:RunInstances one t3.micro in that subnet with TagSpecifications instance tags including {Key: 'sim2real-team', Value: 'blue'}; wait for it to be running.
2. iam:CreateUser; iam:PutUserPolicy with PolicyDocument {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2-instance-connect:SendSSHPublicKey","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/sim2real-team":"green"}}}]}; iam:CreateAccessKey for that user.
3. Using ONLY those credentials, call ec2-instance-connect:SendSSHPublicKey(InstanceId=<the blue instance>, InstanceOSUser='ec2-user', SSHPublicKey=<any format-valid ssh-rsa key, 80..4096 chars>).
4. Observe HTTP 200 with Success=true. AWS refuses with 403 AccessDeniedException and the 'because no identity-based policy allows the ec2-instance-connect:SendSSHPublicKey action' message, because the instance's sim2real-team tag is 'blue', not 'green'.
5. Then ec2:CreateTags(Resources=[<instance>], Tags=[{Key:'sim2real-team', Value:'green'}]) and repeat step 3: this call must now be authorized and return Success=true, so the emulator cannot be fixed by refusing unconditionally — the condition must actually be evaluated against the instance's tags.
