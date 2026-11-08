Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.add-role-to-instance-contract rds.add-role-to-instance-quota-exceeded-rejected rds.instance-iam-role-association-lifecycle rds.concurrent-add-role-to-instance-duplicate-race rds.add-role-to-cluster-quota-exceeded-rejected rds.add-role-to-cluster-contract rds.add-role-without-iam-trust-denied
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.add-role-to-instance-contract|_error_detail#21|AssertionError|raise AssertionError("the call succeeded; expected a rejection")
Issue-Key: rds.add-role-to-instance-quota-exceeded-rejected|run#15|AssertionError|assert code == "InvalidParameterValue", f"unexpected error code {code!r}: {message!r}"
Issue-Key: rds|AddRoleToDBInstance|DBInstanceRoleAlreadyExists
Issue-Key: rds.concurrent-add-role-to-instance-duplicate-race|run#26|AssertionError|assert loser in refusals, f"unexpected loser refusal {loser!r} (expected one of {refusals})"
Issue-Key: rds.add-role-to-cluster-quota-exceeded-rejected|run#17|AssertionError|assert message == expected, f"unexpected message: {message!r} (expected {expected!r})"
Issue-Key: rds.add-role-to-cluster-contract|run#16|AssertionError|assert message == f"DBCluster not found: {bogus_id}", message
Issue-Key: rds.add-role-without-iam-trust-denied|run#13|AssertionError|assert message == expected, f"unexpected message: {message!r} (expected {expected!r})"
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# rds: 5 net-new causes in the IAM-role-association surface — a malformed RoleArn is accepted, and every AddRoleToDB{Instance,Cluster} refusal carries an emulator-authored code or message

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBInstance

## Description
This issue bundles 5 net-new root causes in the rds service's IAM-role-association surface (AddRoleToDBInstance / AddRoleToDBCluster). Every one is grounded in a failing behavioural check that the suite report marked neither ALREADY FILED nor OWNED. A fixer must close all FIVE causes below before submitting.

=== Cause 1 of 5: AddRoleToDBInstance accepts a malformed RoleArn ===
AWS: Smithy: AddRoleToDBInstance input members DBInstanceIdentifier, RoleArn and FeatureName are all required; RoleArn is a plain String in this operation's input shape (unlike AddRoleToDBCluster, whose RoleArn targets IAMRoleArn with pattern ^arn:aws[a-z-]*:iam::[0-9]*:role/.*$), so the ARN-form check is the handler's own parameter validation. A value that is not an IAM role ARN is refused 400 InvalidParameterValue with the message "Invalid Role ARN: <RoleArn>" — fixed text "Invalid Role ARN: " followed by the RoleArn exactly as submitted — and this validation fires independently of (and before any effect of) the instance lookup, leaving the instance's existing AssociatedRoles untouched.
Emulator: the call SUCCEEDS. AddRoleToDBInstance(DBInstanceIdentifier=<a real, available postgres instance>, RoleArn="not-a-valid-role-arn", FeatureName="s3Import") returns 200 with no error, so the check's rejection helper raises "the call succeeded; expected a rejection". No InvalidParameterValue is produced for any malformed ARN value.
Why: this is an overly-permissive gate. A client that mistypes or templates a wrong RoleArn (a very common CloudFormation/IaC mistake) gets a success it must not get, silently records a junk association on the instance, and only discovers the breakage later when the S3 import/export feature fails with no attribution. A negative test that depends on AWS refusing bad ARNs passes against the emulator and fails against AWS.
Fix: AddRoleToDBInstance's input validation in the rds crate's AddRoleToDBInstance handler — validate the RoleArn's ARN form before the instance lookup and refuse with InvalidParameterValue.
Traces: rds.add-role-to-instance-contract.

=== Cause 2 of 5: a duplicate (RoleArn, FeatureName) association on a DB INSTANCE is refused with the wrong error code, and the DBInstanceRoleAlreadyExists message is emulator-authored ===
AWS: Smithy: AddRoleToDBInstance models DBInstanceRoleAlreadyExistsFault (awsQueryError code DBInstanceRoleAlreadyExists, HTTP 400) and InvalidParameterValue among its refusals. Recorded behavior: when a feature name already has a role on that instance, AWS refuses 400 InvalidParameterValue with "The engine <EngineDisplayName> supports only one ARN associated with feature name <FeatureName>." — fixed text "The engine ", " supports only one ARN associated with feature name ", ".", with the engine display name (e.g. PostgreSQL) and the submitted FeatureName filled in. Where AWS does answer DBInstanceRoleAlreadyExists instead, its message is "The <RoleArn> IAM role or <FeatureName> feature is already associated with the <DBInstanceIdentifier> DB instance. Check the name and try again." — fixed text around the submitted RoleArn, FeatureName and DBInstanceIdentifier.
Emulator: the duplicate is refused with code 'DBInstanceRoleAlreadyExists' and the message "The IAM role <RoleArn> is already associated with the DB instance <DBInstanceIdentifier> for the feature <FeatureName>." That message is in neither AWS form, and the code is not the InvalidParameterValue AWS uses for the one-ARN-per-feature rule. The same wrong pair is returned for the sequential duplicate and for the loser of two simultaneous AddRoleToDBInstance calls for one (RoleArn, FeatureName).
Why: a client that branches on the error code (retry/ignore on DBInstanceRoleAlreadyExists vs. fail on InvalidParameterValue) takes the wrong branch, and any operator runbook or log matcher keyed on AWS's sentence never matches. The emulator's message also misrepresents the rule as "this role is already attached" rather than "this engine permits only one ARN per feature name".
Fix: the duplicate-association refusal in the rds crate's AddRoleToDBInstance handler — pick the modelled code AWS uses for the one-ARN-per-feature rule and emit AWS's message template for whichever code is returned.
Traces: rds.add-role-to-instance-quota-exceeded-rejected, rds.instance-iam-role-association-lifecycle, rds.concurrent-add-role-to-instance-duplicate-race.

=== Cause 3 of 5: the duplicate-association refusal on a DB CLUSTER carries an emulator-authored message ===
AWS: Smithy: AddRoleToDBCluster models DBClusterRoleAlreadyExistsFault (awsQueryError code DBClusterRoleAlreadyExists, HTTP 400). Recorded behavior: the message is "Role ARN <RoleArn> is already associated with DB Cluster: <DBClusterIdentifier>. Verify your role ARN and try again." — fixed text "Role ARN ", " is already associated with DB Cluster: ", ". Verify your role ARN and try again.", with the submitted RoleArn and DBClusterIdentifier filled in.
Emulator: the code matches but the message is "The IAM role <RoleArn> is already associated with the DB cluster <DBClusterIdentifier> for the feature <FeatureName>." — different fixed text, and it adds a FeatureName clause AWS's cluster-side message does not carry.
Why: the error message is part of the contract a client reads and logs; a client (or a test) that asserts on AWS's sentence, or that parses it to tell "already associated" from "quota exceeded", sees text AWS never emits.
Fix: the duplicate-association refusal message in the rds crate's AddRoleToDBCluster handler.
Traces: rds.add-role-to-cluster-quota-exceeded-rejected.

=== Cause 4 of 5: AddRoleToDBCluster's not-found refusal message is a different sentence from AWS's ===
AWS: Smithy: AddRoleToDBCluster models DBClusterNotFoundFault (awsQueryError code DBClusterNotFoundFault, HTTP 404) — "DBClusterIdentifier doesn't refer to an existing DB cluster." Recorded behavior: the message is "DBCluster not found: <DBClusterIdentifier>" — fixed text "DBCluster not found: " followed by the identifier exactly as submitted.
Emulator: the code is right but the message is "The source cluster could not be found or cannot be accessed: <DBClusterIdentifier>" — AWS's wording for a *source* cluster in replication/clone paths, not the AddRoleToDBCluster not-found sentence.
Why: a client cannot distinguish "this cluster does not exist" from "the source cluster of a clone/replica cannot be accessed", and anything matching AWS's text (log alarms, CFN custom resources, support runbooks) misclassifies the failure.
Fix: the not-found refusal message in the rds crate's AddRoleToDBCluster handler (and the shared DBClusterNotFoundFault message builder it uses, so the source-cluster sentence is not reused for a plain lookup miss).
Traces: rds.add-role-to-cluster-contract.

=== Cause 5 of 5: the untrusted-role refusal message is emulator-authored ===
AWS: Smithy: AddRoleToDBCluster's refusals include InvalidParameterValue; the role's trust policy must allow rds.amazonaws.com to assume it. Recorded behavior: a role whose trust policy names another service principal (e.g. ec2.amazonaws.com) is refused synchronously with 400 InvalidParameterValue and the message "IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION" — that sentence is entirely fixed text; AWS deliberately does NOT echo the role ARN or name the missing trust principal here.
Emulator: the code matches but the message is "The IAM role <RoleArn> cannot be assumed by Amazon RDS. Add rds.amazonaws.com to the trust relationship of the role and try again." — a different, more informative sentence that AWS does not emit, and it interpolates the request's RoleArn into text AWS keeps constant.
Why: a client or test that asserts AWS's fixed sentence (the only stable thing about this refusal) does not match, and a client that learns from the emulator to expect the role ARN in the message breaks against AWS.
Fix: the trust-policy validation refusal message in the rds crate's AddRoleToDBCluster handler (and the AddRoleToDBInstance handler, which shares the rule).
Traces: rds.add-role-without-iam-trust-denied.

## Evidence
Cause 1: AssertionError: the call succeeded; expected a rejection — AddRoleToDBInstance(RoleArn="not-a-valid-role-arn") against a real available postgres instance returned 200. Expected 400 InvalidParameterValue "Invalid Role ARN: not-a-valid-role-arn".
Cause 2: AssertionError: unexpected error code 'DBInstanceRoleAlreadyExists': 'The IAM role <roleArn> is already associated with the DB instance <instanceId> for the feature s3Import.' — expected InvalidParameterValue "The engine PostgreSQL supports only one ARN associated with feature name s3Export."; the concurrency check accepted either that or DBInstanceRoleAlreadyExists with "The <roleArn> IAM role or s3Export feature is already associated with the <instanceId> DB instance. Check the name and try again." and the emulator matched neither.
Cause 3: AssertionError: unexpected message: 'The IAM role <roleArn> is already associated with the DB cluster <clusterId> for the feature s3Import.' (expected 'Role ARN <roleArn> is already associated with DB Cluster: <clusterId>. Verify your role ARN and try again.')
Cause 4: AssertionError: 'The source cluster could not be found or cannot be accessed: <clusterId>' — expected 'DBCluster not found: <clusterId>'.
Cause 5: AssertionError: unexpected message: 'The IAM role <roleArn> cannot be assumed by Amazon RDS. Add rds.amazonaws.com to the trust relationship of the role and try again.' (expected 'IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION')

## Steps to Reproduce
Cause 1: CreateDBInstance(Engine=postgres, DBInstanceClass=db.t3.micro, AllocatedStorage=20, a DB subnet group), wait available, AddRoleToDBInstance with a real role (succeeds), then AddRoleToDBInstance(DBInstanceIdentifier=<that instance>, RoleArn="not-a-valid-role-arn", FeatureName="s3Import") — must be refused 400 InvalidParameterValue "Invalid Role ARN: not-a-valid-role-arn" and must not change AssociatedRoles.
Cause 2: on the same instance, AddRoleToDBInstance(RoleArn=R, FeatureName="s3Import") twice (and again with two simultaneous calls for one (R, s3Export)) — the duplicate must be refused with AWS's code/message pair for the one-ARN-per-feature rule.
Cause 3: CreateDBCluster(Engine=aurora-postgresql), wait available, AddRoleToDBCluster(RoleArn=R, FeatureName="s3Import") twice — read the second call's Error.Message.
Cause 4: AddRoleToDBCluster(DBClusterIdentifier=<an identifier that does not exist>, RoleArn=<any role>, FeatureName="s3Import") — read Error.Code/Error.Message.
Cause 5: CreateRole with a trust policy allowing only ec2.amazonaws.com, then AddRoleToDBCluster(DBClusterIdentifier=<an available aurora-postgresql cluster>, RoleArn=<that role>, FeatureName="s3Import") — read Error.Message.
