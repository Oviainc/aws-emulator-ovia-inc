Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-server-user-sshkey-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-server-user-sshkey-lifecycle transfer-user-lifecycle-and-list-users transfer-list-users-on-server-workflow transfer-user-lifecycle-visible-then-404-after-delete transfer-list-users-pagination
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeUser|2xx->4xx|$.ServerId
Issue-Key: transfer|DescribeUser|2xx->4xx|$.User
Issue-Key: transfer|DescribeUser|2xx->4xx|status
Issue-Key: transfer|ListUsers|2xx->2xx|$.Users[*]
Issue-Key: transfer|ListUsers|2xx->2xx|$.NextToken
Issue-Key: transfer|DeleteUser|2xx->4xx|status
Issue-Key: transfer|DeleteUser|2xx->4xx|$
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# Transfer user state is not persisted: created users invisible to DescribeUser, ListUsers and DeleteUser

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUser

## Description
A user created with CreateUser on an existing server is accepted (CreateUser answers 200) but no subsequent read or mutate path on the emulator can find it: DescribeUser, ListUsers and DeleteUser all behave as though the server has no users. This is one defect in the transfer user state store (CreateUser does not commit the user into the server's user collection, or commits it under a key the read paths do not look up), surfacing in 6 divergence clusters. There is no open transfer issue; the only existing transfer issue is the already-resolved bootstrap issue.

Cause 1 of 6 — DescribeUser does not see a just-created user.
AWS: Smithy: DescribeUser input {ServerId (required, ^s-([0-9a-f]{17})$), UserName (required)}, output {ServerId (required), User: DescribedUser (required)}; ResourceNotFoundException is modeled only for a user that does not exist. Recorded behavior: after CreateUser succeeds, DescribeUser returns 200 with ServerId and a User carrying Arn, HomeDirectory, HomeDirectoryType, Role, SshPublicKeys and Tags for that UserName.
Emulator: answers HTTP 404 with __type ResourceNotFoundException; ServerId and User are absent.
Why: a client that creates a user and then reads it back (the normal provision-then-verify sequence, and what every SDK waiter/CLI script does) is told the resource does not exist, so provisioning code treats a successful create as failed.
Fix: the transfer CreateUser handler / the service's user state store — the created user must be recorded against its ServerId so DescribeUser resolves it.
Traces: transfer-server-user-sshkey-lifecycle#task#4, also transfer-server-user-sshkey-lifecycle#task#8 and transfer-user-lifecycle-and-list-users#task#7.

Cause 2 of 6 — ListUsers returns an empty Users list on a server that has one user.
AWS: Smithy: ListUsers output {ServerId (required), Users: ListedUsers (required), NextToken}. Recorded behavior: Users contains one ListedUser per created user with Arn, HomeDirectory, HomeDirectoryType, Role, SshPublicKeyCount and UserName.
Emulator: returns 200 but Users[0] is missing (the list is empty).
Why: a client enumerating a server's users sees none, so list-then-act workflows (reconciliation, cleanup, membership checks) silently no-op.
Fix: the transfer ListUsers handler must enumerate the users held in state for the requested ServerId.
Traces: transfer-list-users-on-server-workflow#task#4, also #task#5 and transfer-user-lifecycle-visible-then-404-after-delete#task#5.

Cause 3 of 6 — ListUsers returns an empty Users list on a server that has two users.
AWS: as Cause 2; recorded behavior shows both created users present in Users, each with Arn/HomeDirectory/HomeDirectoryType/Role/SshPublicKeyCount/UserName.
Emulator: returns 200 with Users[0] and Users[1] both missing.
Why: same as Cause 2, and it also shows the gap is not a one-element edge case but the whole enumeration.
Fix: same component as Cause 2.
Traces: transfer-user-lifecycle-and-list-users#task#6.

Cause 4 of 6 — paginated ListUsers produces neither a page of users nor a NextToken.
AWS: Smithy: ListUsers is @paginated(inputToken: NextToken, outputToken: NextToken, items: Users, pageSize: MaxResults), MaxResults @range 1..1000. Recorded behavior: a bounded first page returns the first user(s) in Users and a non-empty NextToken (value itself is environment-specific and only asserted as present).
Emulator: returns 200 with Users empty and NextToken absent, so pagination terminates immediately.
Why: paginators relying on NextToken conclude the server has no users after one call; a client can never walk a multi-page user set.
Fix: the transfer ListUsers handler's pagination (page slicing plus token emission) over the user state store.
Traces: transfer-list-users-pagination#task#6.

Cause 5 of 6 — DeleteUser on a user that was just created answers not-found.
AWS: Smithy: DeleteUser input {ServerId, UserName} both required, output Unit (empty body), @idempotent. Recorded behavior: deleting an existing user returns HTTP 200 with an empty body.
Emulator: answers HTTP 404.
Why: teardown and reconciliation code cannot delete users it created; the resource is unreachable in both directions.
Fix: same user state store as Cause 1 — DeleteUser must resolve the user recorded by CreateUser.
Traces: transfer-list-users-on-server-workflow#teardown#6, also transfer-list-users-pagination#teardown#9/#10/#11 and transfer-user-lifecycle-and-list-users#teardown#8/#10.

Cause 6 of 6 — DeleteUser answers a ResourceNotFoundException body where AWS returns an empty 200.
AWS: as Cause 5 — HTTP 200 and an empty JSON body {} for a user that exists.
Emulator: HTTP 404 with a body carrying __type ResourceNotFoundException plus Resource/ResourceType members (the recorded message text is only an example).
Why: the delete half of the user lifecycle fails after SSH-key operations on the same user, so the lifecycle can never be completed or re-run cleanly.
Fix: same component as Cause 5.
Traces: transfer-server-user-sshkey-lifecycle#task#10.

## Evidence
Cause 1: transfer-server-user-sshkey-lifecycle#task#4 DescribeUser — expected 200 with $.ServerId='s-00000000000000001' and $.User={Arn, HomeDirectory '/probe', HomeDirectoryType 'PATH', Role, SshPublicKeys [], Tags [], UserName}; got 404, $.ServerId and $.User missing.
Cause 2: transfer-list-users-on-server-workflow#task#4 ListUsers — expected $.Users[0]={Arn .../user/s-00000000000000001/<name>, HomeDirectory '/', HomeDirectoryType 'PATH', Role, SshPublicKeyCount 0, UserName}; got $.Users[0] missing (status 200 both).
Cause 3: transfer-user-lifecycle-and-list-users#task#6 ListUsers — expected $.Users[0] and $.Users[1] (two users); both missing.
Cause 4: transfer-list-users-pagination#task#6 ListUsers — expected $.NextToken present (shaped, not value-asserted) and $.Users[0]; both missing.
Cause 5: transfer-list-users-on-server-workflow#teardown#6 DeleteUser — expected status 200, got 404 (6 steps in cluster).
Cause 6: transfer-server-user-sshkey-lifecycle#task#10 DeleteUser — expected status 200 with body {}, got 404 with {'__type': 'ResourceNotFoundException', 'Message': ..., 'Resource': ..., 'ResourceType': 'user'}.

## Steps to Reproduce
1. CreateServer (Domain S3, EndpointType PUBLIC, Protocols [SFTP], IdentityProviderType SERVICE_MANAGED) and note the ServerId.
2. CreateUser on that ServerId with a Role ARN and HomeDirectory '/': observe 200.
3. DescribeUser with the same ServerId/UserName -> emulator answers 404 instead of 200 with the User (Cause 1).
4. ListUsers on that ServerId -> Users is empty instead of containing the user (Causes 2/3); repeat with MaxResults=1 -> no NextToken either (Cause 4).
5. DeleteUser with the same ServerId/UserName -> 404 with a ResourceNotFoundException body instead of 200 {} (Causes 5/6).
Replay: fixtures transfer-server-user-sshkey-lifecycle, transfer-list-users-on-server-workflow, transfer-user-lifecycle-and-list-users, transfer-list-users-pagination.
