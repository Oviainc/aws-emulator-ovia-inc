Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-connector-transfer-request-validation
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-connector-transfer-request-validation
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|StartDirectoryListing|4xx->2xx|$.Message
Issue-Key: transfer|StartDirectoryListing|4xx->2xx|$.__type
Issue-Key: transfer|StartDirectoryListing|4xx->2xx|status
Issue-Key: transfer|StartRemoteDelete|4xx->2xx|$.Message
Issue-Key: transfer|StartRemoteDelete|4xx->2xx|$.__type
Issue-Key: transfer|StartRemoteDelete|4xx->2xx|status
Issue-Commit: 67cdc1f1808266762c6929cf4fb4579090c6bf9c

# Transfer AS2 connector protocol gate missing: StartDirectoryListing and StartRemoteDelete answer 200 instead of refusing

> _Note: this issue was identified against the code base at commit `67cdc1f1808266762c6929cf4fb4579090c6bf9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDirectoryListing

## Description
Two net-new divergences in the transfer emulator share one fault: the connector-protocol gate is not applied to the SFTP-only remote-filesystem operations. StartDirectoryListing and StartRemoteDelete are SFTP-connector operations; when they are invoked against an AS2 connector AWS refuses them with InvalidRequestException, while the emulator performs the call and answers 200 with a freshly allocated id. Each cause must be closed before submitting; there are TWO.

Note on status: the recorded corpus returns HTTP 400 for this service's InvalidRequestException envelope (the model declares InvalidRequestException httpError 400), so 400 is both the modelled and recorded requirement here.

Related but NOT covering: the open issue 'Transfer refusal paths diverge: nine operations answer emulator-authored messages, the wrong error code, or no refusal at all' (20261029T184519Z-transfer-refusal-paths-diverge-nine-operations-answer-emulator.md) carries the SAME fault for StartRemoteMove and ListFileTransferResults, but its Issue-Key set and Issue-Trace-Ids cover neither StartDirectoryListing nor StartRemoteDelete, so neither cause below is exercised by its replay pointer. A fixer closing that issue will leave both operations ungated.

Cause 1 of 2 - StartDirectoryListing against an AS2 connector: NO REFUSAL AT ALL.
AWS: Smithy: StartDirectoryListing(ConnectorId required @length 19 @pattern ^c-([0-9a-f]{17})$, RemoteDirectoryPath required, OutputDirectoryPath required, MaxItems) -> ListingId required + OutputFileName required; modelled errors include InvalidRequestException (errorKind client, httpError 400) and ResourceNotFoundException. Directory listing is a remote-filesystem operation available only on SFTP connectors. Recorded response: HTTP 400, __type 'InvalidRequestException', Message 'Directory listing not supported for AS2 connectors'. The whole message is FIXED text for this error class - it has no variable slot (it names the operation's capability and the connector protocol, not the request's connector id or paths).
Emulator: HTTP 200 with a populated StartDirectoryListingResponse (ListingId/OutputFileName); no __type and no Message are emitted.
Why: a client believes a listing was started on a connector that cannot perform one, then polls or fetches an output file that will never be produced; an unsupported-protocol bug in the caller is masked entirely instead of being reported at the call that caused it.
Fix: StartDirectoryListing's handler - gate on the referenced connector's protocol (AS2 vs SFTP) BEFORE allocating a ListingId, and raise InvalidRequestException with the service's fixed text.
Traces: transfer-connector-transfer-request-validation#task#8.

Cause 2 of 2 - StartRemoteDelete against an AS2 connector: NO REFUSAL AT ALL.
AWS: Smithy: StartRemoteDelete(ConnectorId required @length 19 @pattern ^c-([0-9a-f]{17})$, DeletePath required @length 1..1024) -> DeleteId required; modelled errors include InvalidRequestException (errorKind client, httpError 400) and ResourceNotFoundException. Remote delete is likewise an SFTP-connector-only operation. Recorded response: HTTP 400, __type 'InvalidRequestException', Message 'Remote delete not supported for AS2 connectors' - again entirely fixed text for this error class, with no variable part taken from the request or state.
Emulator: HTTP 200 with a DeleteId; no __type and no Message.
Why: this is the destructive member of the pair - the caller is told a delete was accepted against a connector that cannot delete anything, so a cleanup or retention routine records success for a file that still exists, and nothing surfaces that the operation is unsupported for the connector's protocol.
Fix: StartRemoteDelete's handler - same connector-protocol gate, applied before allocating a DeleteId. Both operations (and the already-filed StartRemoteMove/ListFileTransferResults) should share one gate on the connector's protocol rather than each handler re-deriving it.
Traces: transfer-connector-transfer-request-validation#task#10.

## Evidence
Both causes come from replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 85.4%, 28 diverged steps, 25 divergence clusters). Neither cluster carries a stub_path and neither carries an owned_by, i.e. no open issue holds their replay key. Expected = recorded AWS response, Actual = emulator.

1. StartDirectoryListing / transfer-connector-transfer-request-validation#task#8 - status: expected 400, got 200; $.__type: expected 'InvalidRequestException', got None (missing); $.Message: expected 'Directory listing not supported for AS2 connectors', got None (missing). Divergence key: service=transfer|StartDirectoryListing|status=400->200|keys=$.Message,$.__type,status.

2. StartRemoteDelete / transfer-connector-transfer-request-validation#task#10 - status: expected 400, got 200; $.__type: expected 'InvalidRequestException', got None (missing); $.Message: expected 'Remote delete not supported for AS2 connectors', got None (missing). Divergence key: service=transfer|StartRemoteDelete|status=400->200|keys=$.Message,$.__type,status.

Message anatomy for both: the entire string is fixed text belonging to the error class; there are no request- or state-derived slots, so the expected strings above are the requirement verbatim.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): StartDirectoryListing declares errors InternalServiceError(503), InvalidRequestException(400), ResourceNotFoundException(404), ServiceUnavailableException(503), ThrottlingException(429); StartRemoteDelete declares the identical error set. Both inputs require ConnectorId with @pattern ^c-([0-9a-f]{17})$, and both outputs require a generated id (ListingId / DeleteId) - which is exactly what the emulator returns in place of the refusal.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay the two bound steps:
   - transfer-connector-transfer-request-validation#task#8 (StartDirectoryListing against the fixture's AS2 connector)
   - transfer-connector-transfer-request-validation#task#10 (StartRemoteDelete against the same AS2 connector)
4. For each, compare the HTTP status, $.__type and $.Message against the recorded response: both must be HTTP 400 with __type 'InvalidRequestException' and the fixed Message text ('Directory listing not supported for AS2 connectors' / 'Remote delete not supported for AS2 connectors'), not a 200 carrying a ListingId/OutputFileName or a DeleteId.
5. Confirm the earlier steps of the same fixture still pass (the AS2 connector is created in that fixture before these calls), so the gate must key on the connector's protocol rather than rejecting the operations outright.
