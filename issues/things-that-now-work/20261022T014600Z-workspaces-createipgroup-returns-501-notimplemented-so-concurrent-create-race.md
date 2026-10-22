Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces.concurrent-ip-group-creation-race
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces.concurrent-ip-group-creation-race|run#9|AssertionError|assert successes, "expected at least one winner, got none"
Issue-Commit: c6f9974c04d0d99d1dbb9b00d2d222f6d449381b

# CreateIpGroup returns 501 NotImplemented, so concurrent-create race leaves zero accepted groups and no single-survivor state

> _Note: this issue was identified against the code base at commit `c6f9974c04d0d99d1dbb9b00d2d222f6d449381b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIpGroup

## Description
Cause 1 of 1: every CreateIpGroup call is answered by the scaffold stub with a NotImplemented error instead of creating an IP access control group, so no concurrent-creation outcome (accept, duplicate-name rejection, single persisted survivor) can be produced at all.

AWS: Smithy (com-amazonaws-workspaces-workspacesservice) models CreateIpGroup with required input member GroupName (target IpGroupName), optional GroupDesc, UserRules and Tags, and output member GroupId (target IpGroupId, pattern ^wsipg-[0-9a-z]{8,63}$). Its modeled errors are AccessDeniedException, InvalidParameterValuesException, ResourceAlreadyExistsException, ResourceCreationFailedException and ResourceLimitExceededException, all client errors with HTTP 400. NotImplemented is not among them. For the concurrency behavior itself the model says nothing — recorded behavior only: when N concurrent CreateIpGroup calls share one GroupName, at least one call is accepted with HTTP 200 and a distinct GroupId, every rejected call gets ResourceAlreadyExistsException (HTTP 400), and a subsequent DescribeIpGroups shows exactly one persisted group under that name whose groupId is one of the accepted ids.

Emulator: all 8 concurrent CreateIpGroup calls fail with code NotImplemented (message example: "operation CreateIpGroup is scaffolded but not implemented in this emulator"). The check's own error handler swallows these, so the replay verdict is a mismatch rather than an error: successes is empty and the assertion "expected at least one winner, got none" fires. Nothing is persisted, so the terminal DescribeIpGroups read finds no group under the name and the single-survivor invariant cannot hold either.

Why: a client creating an IP access control group gets an unmodeled NotImplemented error instead of a GroupId, so no IP-access-control configuration flow works. Clients that race creates on a shared name (idempotent provisioners, Terraform/CDK retries) neither receive a usable GroupId nor the ResourceAlreadyExistsException they branch on, and they cannot observe the one group that AWS would leave persisted.

Fix: the CreateIpGroup handler and the IP-group state it writes (group registry keyed by GroupName for the uniqueness/ResourceAlreadyExistsException branch, GroupId minting to the modeled wsipg- pattern) so that concurrent creates on one name yield at least one 200 and exactly one persisted group visible to DescribeIpGroups.

Related: the same unimplemented CreateIpGroup stub is already filed as the scaffold-stub issue naming services/service-com-amazonaws-workspaces-workspacesservice/src/ops/create_ip_group.rs (it covers the error-verdict checks blocked on that operation). This issue is filed separately because no open issue carries the replay key for workspaces.concurrent-ip-group-creation-race — that check swallows the ClientError, so it reports as a mismatch and was not attributed to the stub — and because the concurrency/single-survivor invariant above must be re-replayed before this is closed.

Traces: workspaces.concurrent-ip-group-creation-race.

## Evidence
Expected (AWS, recorded behavior): 8 concurrent CreateIpGroup(GroupName=<shared name>, GroupDesc="race", Tags=[...]) calls -> at least one HTTP 200 carrying a GroupId matching ^wsipg-[0-9a-z]{8,63}$; every rejected call -> HTTP 400 with error code ResourceAlreadyExistsException; a following DescribeIpGroups pagination sweep -> exactly one entry with groupName == <shared name> whose groupId is one of the accepted ids.

Actual (emulator): all 8 calls fail with error code NotImplemented (example message: "operation CreateIpGroup is scaffolded but not implemented in this emulator"); successes == [] and failures == ["NotImplemented"] x8. Replay verdict: [mismatch] workspaces.concurrent-ip-group-creation-race, AssertionError at check_workspaces_concurrent_ip_group_creation_race.py line 54: "expected at least one winner, got none". No group is persisted, so the terminal single-survivor read also cannot be satisfied.

## Steps to Reproduce
1. Boot the emulator and point a botocore workspaces client at it.
2. Call create_ip_group(GroupName="sim2real-wsipg-race-<unique>", GroupDesc="race", Tags=[{"Key":"sim2real-behav","Value":"true"}]) 8 times concurrently (ThreadPoolExecutor, max_workers=8).
3. Observe every call raises ClientError with code NotImplemented instead of returning a GroupId, and that none is a ResourceAlreadyExistsException.
4. Page describe_ip_groups() to exhaustion and observe no entry has groupName == the shared name, so no single survivor exists.
5. Equivalently, replay check workspaces.concurrent-ip-group-creation-race from the behavioral suite.
