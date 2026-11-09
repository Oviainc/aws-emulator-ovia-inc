Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__memorydb.deploy-memorydb-user-and-acl-round-trip-then-delete cloudformation__ec2__memorydb.deploy-memorydb-user-password-too-short-rejected
Issue-Key: cloudformation__ec2__memorydb.deploy-memorydb-user-and-acl-round-trip-then-delete|run#15|AssertionError|assert len(listed_users) == 1, f"DescribeUsers found {listed_users!r} for name {user!r}"
Issue-Key: cloudformation__ec2__memorydb.deploy-memorydb-user-password-too-short-rejected|run#9|AssertionError|assert good == [("active", _ACCESS_STRING, "password")], f"control user reads {good!r}"
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# cloudformation__memorydb: an AWS::MemoryDB::User provisioned by a CREATE_COMPLETE stack does not exist in MemoryDB — DescribeUsers never lists it, so no CFN-deployed MemoryDB user or ACL is readable

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUsers

## Description
Cause 1 of 1: a stack declaring AWS::MemoryDB::User reaches CREATE_COMPLETE and its Outputs resolve, but the user is never materialized in MemoryDB's own state: DescribeUsers with no UserName filter (the full, paginated list) contains no row whose Name is the templated UserName. Two checks fail at exactly this point — the user/ACL round-trip, and the positive control of the short-password rejection check.

AWS: Smithy com.amazonaws.memorydb#DescribeUsers takes an optional UserName (pattern ^[a-zA-Z][a-zA-Z0-9\\-]*$) and returns Users: UserList, paginated on NextToken/MaxResults; CreateUser takes UserName, AuthenticationMode and AccessString and returns the created User. CloudFormation's AWS::MemoryDB::User resource is provisioned through that CreateUser call, so once the stack is CREATE_COMPLETE the user is a first-class MemoryDB resource: an unfiltered DescribeUsers lists exactly one row with Name = the templated UserName, ARN = arn:<partition>:memorydb:<region>:<account>:user/<user-name>, AccessString echoing the templated AccessString, Authentication.Type "password" and Status "active"; DescribeUsers(UserName=<that name>) returns it, ListTags on its ARN returns the templated tags plus CloudFormation's aws: system tags, and an AWS::MemoryDB::ACL listing it under UserNames is likewise readable through DescribeACLs with Status "active". The variable parts above (user name, ACL name, partition, region, account) come from the request and the caller's identity — no literal name from one run is part of the requirement.

Emulator: the CREATE_COMPLETE stack's user is invisible to MemoryDB. DescribeUsers' full list, filtered by the templated name, is empty ('DescribeUsers found [] for name <user>'), and in the control stack the derived (Status, AccessString, Authentication.Type) tuple list reads [] instead of [("active", "on ~* &* +@all", "password")]. Because the user never exists, every downstream assertion of these scenarios is unreachable: the Ref/GetAtt Arn cross-check against DescribeUsers, the user's tags, the ACL's UserNames/Status through DescribeACLs, the ACL-dropping update leaving the user active, and the post-delete UserNotFoundFault refusal.

Why: a client that deploys MemoryDB users and ACLs as IaC and then drives MemoryDB's own API with them gets a green stack and an empty account — DescribeUsers/DescribeACLs cannot see the resources the stack claims to own, so credential/ACL wiring (attaching the ACL to a cluster, rotating the user's password, auditing who exists) silently operates on nothing. Worse, the stack reports success, so there is no signal to retry or fail.

Fix: CloudFormation's AWS::MemoryDB::User and AWS::MemoryDB::ACL resource provisioners must create, update and delete through MemoryDB's own state (the CreateUser/UpdateUser/DeleteUser and CreateACL/UpdateACL/DeleteACL paths and the state they write), rather than recording a stack-local resource; the MemoryDB DescribeUsers / DescribeACLs read paths must then list them, including CloudFormation's propagated aws: tags on ListTags. Whatever the provisioner uses to mint the Ref/GetAtt Arn must be the same record DescribeUsers reads, so the two cannot disagree.

Related but NOT covering: open issue 20261107T085614Z-apigateway-dynamodb-lambda-logs-a-cloudformation-created-dynamodb-table-is-not-addressable.md and 20261109T113512Z-cloudformation-network-firewall-aws-networkfirewall-firewallpolicy-and-rulegroup.md describe the same class of defect for DynamoDB tables and Network Firewall resources; neither covers AWS::MemoryDB::User/::ACL or carries these replay keys.

Traces: cloudformation__ec2__memorydb.deploy-memorydb-user-and-acl-round-trip-then-delete, cloudformation__ec2__memorydb.deploy-memorydb-user-password-too-short-rejected.

## Evidence
Expected (user/ACL round-trip): after CREATE_COMPLETE, the unfiltered DescribeUsers list contains exactly one row with Name == the templated UserName, that row's Name equals the stack Output for {"Ref": "User"}, its ARN equals the stack Output for {"Fn::GetAtt": ["User", "Arn"]} and equals arn:<partition>:memorydb:<region>:<account>:user/<user-name>, AccessString == "on ~* &* +@all", Authentication.Type == "password", Status == "active".
Actual: AssertionError: DescribeUsers found [] for name '<templated user name>' — the list is empty.

Expected (short-password control stack): the 16-character-password control stack's user reads [("active", "on ~* &* +@all", "password")].
Actual: AssertionError: control user reads [] — the control stack also reached CREATE_COMPLETE with no user in MemoryDB, so the check never gets to the 8-character-password rejection it exists to assert.

## Steps to Reproduce
Replay the behavioral suite and re-run the check ids in trace_ids. Minimal repro: CreateStack with Parameters {"UserPassword": {"Type": "String", "NoEcho": true}} and Resources {"User": {"Type": "AWS::MemoryDB::User", "Properties": {"UserName": <name>, "AccessString": "on ~* &* +@all", "AuthenticationMode": {"Type": "password", "Passwords": [{"Ref": "UserPassword"}]}, "Tags": [...]}}, "Acl": {"Type": "AWS::MemoryDB::ACL", "Properties": {"ACLName": <acl>, "UserNames": [{"Ref": "User"}]}}}, passing a 16+ character UserPassword. Poll DescribeStacks to CREATE_COMPLETE, then call memorydb DescribeUsers with NO UserName and page every result, and memorydb DescribeACLs with no ACLName: neither list contains the templated names.
