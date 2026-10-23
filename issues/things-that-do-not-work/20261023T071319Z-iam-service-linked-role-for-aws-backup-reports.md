Issue-Source: trace-sweep-divergence
Issue-Trace: backup-report-jobs-listing
Issue-Step: setup#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-report-jobs-listing
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: iam|GetRole|2xx->4xx|$
Issue-Key: iam|GetRole|2xx->4xx|status
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# IAM service-linked role for AWS Backup reports never exists: GetRole answers 404 NoSuchEntity forever

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRole

## Description
Cause 1 of 1: IAM never materializes the service-linked role AWSServiceRoleForBackupReports, so GetRole on it never converges to 200 and every consumer that waits for the role to become visible times out.

AWS: Smithy: GetRole input RoleName (roleNameType, required, @length 1-64, pattern ^[\w+=,.@-]+$); output Role is required; modeled errors NoSuchEntityException (awsQueryError NoSuchEntity, HTTP 404) and ServiceFailureException (500). The model also binds CreateServiceLinkedRole (input AWSServiceName), which is how a service-linked role such as AWSServiceRoleForBackupReports comes into existence in an account. In the recording, GetRole on AWSServiceRoleForBackupReports answers HTTP 200 with the role, which is why the fixture polls it as a prerequisite: once the service-linked role exists, GetRole must resolve it like any other role, and the 404/NoSuchEntity path is reserved for a role name that is genuinely absent.

Emulator: answers HTTP 404 for the whole polling window — 13 attempts, capped at 60s — with the awsQuery error envelope ErrorResponse/Error { Type: Sender, Code: NoSuchEntity, Message example "The role with name AWSServiceRoleForBackupReports cannot be found." }. The role is never created by, nor seeded for, the Backup-reports path, so the poll can never succeed.

Why: a client that provisions AWS Backup reporting follows AWS's documented pattern of ensuring the service-linked role and then waiting for IAM to return it before using the dependent resource. Against the emulator that wait never terminates: the caller either spins until its own timeout or aborts with a hard NoSuchEntity, and every workflow gated on the role (here the Backup report-jobs listing setup) is blocked before its own assertions ever run. Because the failure is in a prerequisite poll, it masks the behavior of everything downstream of it.

Fix: IAM role state in the iam service crate — service-linked roles must become resolvable by GetRole, either by having CreateServiceLinkedRole insert AWSServiceRoleForBackupReports into the role store under the name IAM uses (and the dependent Backup-reports path invoke/auto-create it), or by seeding the service-linked roles the emulator's own cross-service paths depend on. GetRole's handler should then resolve it normally rather than returning NoSuchEntity.

Traces: backup-report-jobs-listing#setup#5 (GetRole expected 200, got 404 NoSuchEntity on every poll attempt).

## Evidence
Fixture backup-report-jobs-listing, step setup#5, operation GetRole (service iam).
Expected (recorded AWS): status 200 with the Role for RoleName=AWSServiceRoleForBackupReports; no error body.
Actual (emulator): status 404 on every attempt; body {"ErrorResponse": {"Error": {"Type": "Sender", "Code": "NoSuchEntity", "Message": "The role with name AWSServiceRoleForBackupReports cannot be found."}}}. Replay reports "poll capped at 60s" and "poll: status 404 not in [200] after 13 attempt(s)".
Replay divergence_key: service=iam|GetRole|status=200->404|keys=$,status (verdict mismatch, 1 affected step).
Smithy: GetRole errors = NoSuchEntityException (NoSuchEntity, 404) / ServiceFailureException (500); CreateServiceLinkedRole is modeled with AWSServiceName input.
Not covered by any open issue: the open IAM issues concern the AWS-managed policy catalog (GetPolicy on arn:aws:iam::aws:policy/*), DeleteRolePolicy on an existing inline policy, authorization evaluation, and the SimulatePrincipalPolicy/CreateLoginProfile stubs — none of them makes a service-linked role resolvable by GetRole.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture backup-report-jobs-listing; its setup phase reaches step setup#5, which polls iam GetRole with RoleName=AWSServiceRoleForBackupReports until it answers 200.
3. Observe every attempt answers 404 NoSuchEntity (13 attempts / 60s cap) instead of 200 with the role, so the setup never completes.
