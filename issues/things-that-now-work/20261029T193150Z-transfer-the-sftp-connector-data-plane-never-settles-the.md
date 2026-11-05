Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.e2e-server-user-connector-file-transfer-flow transfer.e2e-server-user-file-transfer-flow
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.e2e-server-user-connector-file-transfer-flow|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: transfer.e2e-server-user-file-transfer-flow|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# The SFTP connector data plane never settles: the two end-to-end server+user+connector file-transfer flows make no progress and exhaust the suite's budget

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartFileTransfer

## Description
ONE root cause, grounded in two failing checks. There is ONE cause.

Cause 1 of 1 — the server/connector data plane never reaches an observable terminal state, so an end-to-end StartFileTransfer flow never progresses.
AWS: Smithy: StartServer/DescribeServer (DescribedServer.State -> State enum, reaching ONLINE after StartServer); TestConnection(ConnectorId required) -> {Status, StatusMessage, SftpConnectionDetails} where SftpConnectionDetails carries the probed HostKey and Status becomes 'OK' once the connector can reach the endpoint; StartFileTransfer(ConnectorId required, SendFilePaths @length 1..10, RemoteDirectoryPath) -> TransferId; ListFileTransferResults(ConnectorId, TransferId) -> FileTransferResults (required) whose entries carry StatusCode, FilePath and, on failure, FailureCode/FailureMessage. Recorded behavior: in the real service this chain settles — DescribeServer reports ONLINE, TestConnection eventually returns SftpConnectionDetails.HostKey and then Status='OK', StartFileTransfer's transfer appears in ListFileTransferResults with StatusCode COMPLETED (or FAILED), and the sent bytes land at the remote path. Every one of those is an observable state transition, not a value only a live account produces.
Emulator: both end-to-end checks make no progress and are killed at the suite's 60s per-check budget, having produced no assertion failure at all — i.e. one of the polled conditions above never becomes true. The flows poll only emulator-served state (DescribeServer.State, TestConnection.Status/SftpConnectionDetails.HostKey, ListFileTransferResults[].StatusCode) at 3-10s intervals, so a correctly emulating target settles them in well under the budget. Hypothesis (neither the model nor the replay output states which poll stalls): the connector data plane is accept-only — StartRemoteMove's handler documents that an accepted connector request 'stays QUEUED ... nothing here reports a file as moved' (services/service-com-amazonaws-transfer-transferservice/src/ops/start_remote_move.rs:15-18), and the sibling open issue below reports TestConnection never reaching a reachable endpoint, so the likeliest stalls are TestConnection never yielding a HostKey / Status='OK' and ListFileTransferResults never leaving the non-terminal state.
Why: the whole purpose of the connector plane is to move a file and let the caller observe the outcome. A client that follows AWS's documented flow — start the server, probe the connector, start the transfer, poll the results — hangs forever instead of seeing a terminal StatusCode, so a CI job or an orchestration step modelled on real Transfer Family never completes and cannot distinguish 'still running' from 'never going to run'.
Fix: the connector/server data-plane state machine (crate::state for the server State and ConnectorTransfer records, plus the TestConnection, StartFileTransfer and ListFileTransferResults handlers) must advance its records to terminal states: DescribeServer to ONLINE after StartServer, TestConnection to a populated SftpConnectionDetails and Status='OK' for a reachable stored endpoint, and an accepted transfer to a ListFileTransferResults entry with StatusCode COMPLETED and the sent FilePath (writing the payload to the destination), rather than leaving every accepted request in a non-terminal state forever.
Traces: checks transfer.e2e-server-user-connector-file-transfer-flow and transfer.e2e-server-user-file-transfer-flow. Related OPEN issue, which does not carry either check's key: '20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md' (it owns the sibling timeouts transfer.e2e-connector-remote-directory-flow and transfer.file-transfer-result-status-visibility-ttl and reports TestConnection never reaching a reachable endpoint); if its fix lands first, re-replay the two checks named here before closing this one.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (70/110 passed; transfer 69/109):

[mismatch] transfer.e2e-server-user-connector-file-transfer-flow — Error: Check timed out after 60.0s
[mismatch] transfer.e2e-server-user-file-transfer-flow — Error: Check timed out after 60.0s

Neither check raised an assertion: they stopped inside a poll loop, so some emulator-served condition never became true. The conditions the two flows poll, in order (from the replayed check sources):
  1. transfer.describe_server(ServerId)['Server']['State'] == 'ONLINE' after StartServer (interval 10s)
  2. transfer.test_connection(ConnectorId)['SftpConnectionDetails']['HostKey'] present (interval 10s), then UpdateConnector pins it as a TrustedHostKey
  3. transfer.test_connection(ConnectorId)['Status'] == 'OK' (interval 10s)
  4. transfer.list_file_transfer_results(ConnectorId, TransferId)['FileTransferResults'][0]['StatusCode'] in ('COMPLETED','FAILED') after StartFileTransfer (interval 5-10s), with the check retrying StartFileTransfer up to 3 times and still demanding an eventual COMPLETED
  5. s3.get_object of the transferred key returning the original bytes, and result['FilePath'] == the sent '/<bucket>/<key>'
All five read emulator state only; none asserts a token, account-specific id, clock value or request id.

Supporting source evidence that accepted connector work is never advanced: services/service-com-amazonaws-transfer-transferservice/src/ops/start_remote_move.rs lines 15-18 state that an accepted connector request 'stays QUEUED, like every other operation a connector accepts (crate::state::ConnectorTransfer), and nothing here reports a file as moved'.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): TestConnection output = {ConnectorId, Status, StatusMessage, SftpConnectionDetails}; StartFileTransfer output = TransferId (required); ListFileTransferResults output = FileTransferResults (required, 0..1000) + NextToken, paginated on FileTransferResults.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite and observe that transfer.e2e-server-user-connector-file-transfer-flow and transfer.e2e-server-user-file-transfer-flow are killed at the 60s budget with no assertion failure.
3. Reproduce by hand against the emulator: CreateServer(Protocols=['SFTP'], HostKey=<generated RSA PEM>) -> StartServer -> poll DescribeServer until Server.State == 'ONLINE'.
4. CreateUser on that server with an SSH public key, store {Username, PrivateKey} in a secretsmanager secret, then CreateConnector(Url='sftp://<serverId>.server.transfer.<region>.amazonaws.com', SftpConfig={UserSecretId: <secret arn>}, AccessRole=<role>).
5. Poll TestConnection(ConnectorId) and note whether SftpConnectionDetails.HostKey ever appears and whether Status ever becomes 'OK'.
6. StartFileTransfer(ConnectorId, SendFilePaths=['/<bucket>/<key>'], RemoteDirectoryPath='/<bucket>') and poll ListFileTransferResults(ConnectorId, TransferId): note whether any entry ever reaches StatusCode COMPLETED/FAILED, and whether the object lands at the destination.
7. The first of steps 3, 5 or 6 that never settles is the stalled transition.
