Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-list-users-on-server-workflow
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-list-users-on-server-workflow transfer-user-lifecycle-visible-then-404-after-delete transfer-list-users-pagination transfer-user-lifecycle-and-list-users transfer-server-user-sshkey-lifecycle
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|ListUsers|2xx->2xx|$.Users[*]
Issue-Key: transfer|ListUsers|2xx->2xx|$.NextToken
Issue-Key: transfer|DescribeUser|2xx->4xx|$.ServerId
Issue-Key: transfer|DescribeUser|2xx->4xx|$.User
Issue-Key: transfer|DescribeUser|2xx->4xx|status
Issue-Key: transfer|DeleteUser|2xx->4xx|status
Issue-Key: transfer|DeleteUser|2xx->4xx|$
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Transfer user records are invisible to every user-scoped read and delete after CreateUser

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUsers

## Description
Across six independent divergence clusters in the transfer corpus, a user that was created on an existing server is not observable afterwards: ListUsers reports the server as having no users, and DescribeUser/DeleteUser answer the modeled ResourceNotFoundException for a user name that was just created. CreateUser itself replays clean because its modeled output (CreateUserResponse = {ServerId, UserName}) is nothing but an echo of the request, so a create that never becomes visible in state still satisfies the wire assertion on that step; every later step in the user lifecycle then diverges. This issue bundles SIX causes — all on service `transfer`, all the same visibility fault seen through a different operation. A fixer must close all six and re-replay every fixture#step listed below.

Hypothesis (not stated by the model or the recordings): the user records CreateUser writes into the service store are not reachable by the reads — either the write is not landing in the shared per-service store that the server maps use, or the user-scoped read path keys/scans the map differently from the way CreateUser inserts into it. Server records in the SAME store do survive (DescribeServer/ListServers answer created servers), so the defect is specific to the `users` map, not to state persistence in general.

Cause 1 of 6: ListUsers returns an empty Users list for a server that has users.
  AWS: Smithy ListUsers — output `Users` (ListedUsers) @required, `ServerId` @required; @paginated(inputToken=NextToken, outputToken=NextToken, items=Users, pageSize=MaxResults). A user created with CreateUser on that ServerId is listed, each ListedUser carrying Arn, HomeDirectory, HomeDirectoryType, Role and SshPublicKeyCount.
  Emulator: HTTP 200 with Users empty — `$.Users[0]: missing` (recording shows one ListedUser with HomeDirectory '/', HomeDirectoryType 'PATH', SshPublicKeyCount 0 and the created UserName; emulator returned nothing at index 0).
  Why: a client that creates a user and then lists to confirm or to drive teardown sees an empty server and concludes the create failed; iteration over users (console-style refresh, cleanup loops) silently does nothing.
  Fix: the ListUsers handler's read of the service store, or the CreateUser write it reads back — the user store and its per-server scan in the transfer service state.
  Traces: transfer-list-users-on-server-workflow#task#4 (also task#5, and transfer-user-lifecycle-visible-then-404-after-delete#task#5).

Cause 2 of 6: ListUsers with a page bound returns neither the first page nor a NextToken.
  AWS: Smithy ListUsers — `MaxResults` @range(1..1000), `NextToken` @length(1..6144); with more users stored than MaxResults, the first call answers one page of Users AND a NextToken that resumes the listing.
  Emulator: HTTP 200 with `$.Users[0]: missing` and `$.NextToken: missing` — no page content and therefore no continuation token (the recording asserts a shaped, non-empty NextToken plus the first ListedUser).
  Why: a paginating client terminates after one empty page and never observes any user, so pagination over users is unusable even when the store is later fixed to answer page 1.
  Fix: same user-store read path; the pagination bound/token emission in the ListUsers handler must be driven by the stored users for that server.
  Traces: transfer-list-users-pagination#task#6.

Cause 3 of 6: ListUsers returns empty for a server carrying two distinct users.
  AWS: as Cause 1 — both created users are listed, in the service's stable listing order.
  Emulator: HTTP 200 with `$.Users[0]: missing` and `$.Users[1]: missing` (recording shows two ListedUsers for the server).
  Why: confirms the emptiness is not a single-record edge case — no number of created users becomes visible; multi-user workflows (per-user home directories, bulk teardown) all read an empty server.
  Fix: as Cause 1.
  Traces: transfer-user-lifecycle-and-list-users#task#6.

Cause 4 of 6: DescribeUser answers not-found for an existing user instead of 200 with the User projection.
  AWS: Smithy DescribeUser — output `ServerId` @required and `User` (DescribedUser) @required; errors include ResourceNotFoundException (@httpError(404)) which applies only when the server does not hold that UserName. For a user created by CreateUser the answer is 200 with the DescribedUser (Arn, HomeDirectory, HomeDirectoryType, Role, SshPublicKeys, Tags, UserName).
  Emulator: HTTP 400 with no payload members — `status: expected 200, got 400`, `$.ServerId: missing`, `$.User: missing`. (This service maps ResourceNotFoundException to status 400 by design, per src/state.rs RESOURCE_NOT_FOUND_HTTP_STATUS; the divergence here is that the error is raised at all, not the status it is dressed in.)
  Why: the canonical read-after-create check fails, so any client that creates a user and then describes it to obtain the user's Arn or key list treats a successful create as a failure and cannot proceed.
  Fix: the DescribeUser handler's lookup in the user store (and the CreateUser write it must find).
  Traces: transfer-server-user-sshkey-lifecycle#task#4 (also task#8, and transfer-user-lifecycle-and-list-users#task#7).

Cause 5 of 6: DeleteUser fails with 400 instead of succeeding for an existing user.
  AWS: Smithy DeleteUser — output Unit (an empty 200 body), @idempotent; ResourceNotFoundException applies only to a UserName the named server does not hold. Deleting a user created earlier in the same server's lifetime succeeds.
  Emulator: `status: expected 200, got 400` — the delete is refused for the user that was created in the same fixture.
  Why: teardown cannot release users, so a client's cleanup path errors out and the resources it believes it created leak from its point of view; six recorded teardown steps across three fixtures fail this way.
  Fix: the DeleteUser handler's lookup/removal in the user store (and the CreateUser write it must find).
  Traces: transfer-list-users-on-server-workflow#teardown#6 (also transfer-list-users-pagination#teardown#9/#10/#11 and transfer-user-lifecycle-and-list-users#teardown#8/#10).

Cause 6 of 6: DeleteUser answers a ResourceNotFoundException body where AWS answers an empty 200.
  AWS: as Cause 5 — DeleteUser's output shape is Unit, so a successful delete carries status 200 and an empty JSON body `{}`.
  Emulator: `status: expected 200, got 400` with body `{"__type": "ResourceNotFoundException", "Message": "Unknown user", "Resource": "<the user name>", "ResourceType": "User"}` where the recording has `{}`. The error code ResourceNotFoundException and the 400 status are the requirement violated here (the user exists); the `Message` text is quoted only as an example of what the emulator emits.
  Why: a client deleting a user it just created, in-band in its own workflow, receives a hard not-found error; idempotent retry logic cannot distinguish this from a genuinely absent user.
  Fix: as Cause 5.
  Traces: transfer-server-user-sshkey-lifecycle#task#10.

Related open issues (neither covers these causes): '20261022T011353Z-transfer-server-projection-drops-defaulted-members-pins-an.md' (server projection/state defaults) and '20261022T011353Z-transfer-input-validation-diverges-vpc-createserver-accepted-without.md' (CreateServer/CreateProfile input validation). Both are server- and validation-scoped; the user store is untouched by them.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, bootstrap_signal false, state_reset_failures 0), 6 net-new clusters, all service=transfer:

1) ListUsers — transfer-list-users-on-server-workflow#task#4 (affected 3): expected 200, got 200; $.Users[0]: missing (expected {'Arn': 'arn:aws:transfer:...:user/s-.../<name>', 'HomeDirectory': '/', 'HomeDirectoryType': 'PATH', 'Role': '{{role_arn}}', 'SshPublicKeyCount': 0, 'UserName': '<name>'}, got None).
2) ListUsers — transfer-list-users-pagination#task#6: $.NextToken: missing (expected shaped token, got None); $.Users[0]: missing (expected first ListedUser, got None).
3) ListUsers — transfer-user-lifecycle-and-list-users#task#6: $.Users[0] and $.Users[1] both missing (recording lists two users on the server).
4) DescribeUser — transfer-server-user-sshkey-lifecycle#task#4 (affected 3): status expected 200, got 400; $.ServerId missing (expected the created ServerId); $.User missing (expected DescribedUser with HomeDirectory '/probe', HomeDirectoryType 'PATH', SshPublicKeys [], Tags []).
5) DeleteUser — transfer-list-users-on-server-workflow#teardown#6 (affected 6): status expected 200, got 400.
6) DeleteUser — transfer-server-user-sshkey-lifecycle#task#10: status expected 200, got 400; body expected {} , got {'__type': 'ResourceNotFoundException', 'Message': 'Unknown user', 'Resource': '<user name>', 'ResourceType': 'User'}.

Model basis (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): ListUsers output Users @required + @paginated(items=Users, outputToken=NextToken, pageSize=MaxResults), NextToken @length(1..6144); DescribeUser output ServerId/User @required, ResourceNotFoundException @httpError(404); DeleteUser output Unit, @idempotent; CreateUser output = {ServerId, UserName} only (an echo of the request, which is why the create step replays clean).

Code context for the fix (citations only): services/service-com-amazonaws-transfer-transferservice/src/state.rs:245 (`users: BTreeMap<(String, String), User>`), :268 (`users_of` per-server range scan), src/ops/create_user.rs:297 (`guard.users.insert(key, user)`), src/ops/describe_user.rs:172 (`guard.users.get(&key)`), src/ops/list_users.rs:192 (`guard.users_of(...)`), src/state.rs:100-108 (ResourceNotFoundException answered with status 400).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) and read the transfer clusters.
3. Cause 1: replay fixture transfer-list-users-on-server-workflow, step task#4 — CreateServer, CreateUser, then ListUsers on that ServerId: ListUsers answers 200 with an empty Users list instead of the created user.
4. Cause 2: replay transfer-list-users-pagination, step task#6 — create several users, then ListUsers with MaxResults below that count: neither a first page nor a NextToken comes back.
5. Cause 3: replay transfer-user-lifecycle-and-list-users, step task#6 — create two users, then ListUsers: both entries are missing.
6. Cause 4: replay transfer-server-user-sshkey-lifecycle, step task#4 — CreateUser then DescribeUser with the same ServerId/UserName: 400 ResourceNotFoundException instead of 200 with the User projection.
7. Cause 5: replay transfer-list-users-on-server-workflow, step teardown#6 — DeleteUser for the user created in that fixture: 400 instead of 200.
8. Cause 6: replay transfer-server-user-sshkey-lifecycle, step task#10 — DeleteUser in-band after the create: 400 with a ResourceNotFoundException body where the recording has an empty 200 body.
9. Re-replay all six fixture#step pairs after the fix; each must pass.
