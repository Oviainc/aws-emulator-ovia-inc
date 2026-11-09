Issue-Source: trace-sweep-divergence
Issue-Trace: connectparticipant-customer-message-read-back-from-transcript
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectparticipant-customer-message-read-back-from-transcript connectparticipant-participant-session-lifecycle
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant|GetTranscript|2xx->2xx|$.Transcript[*].MessageMetadata
Issue-Key: connectparticipant|GetTranscript|2xx->2xx|$.NextToken
Issue-Key: connectparticipant|GetTranscript|2xx->2xx|$.Transcript[*]
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# GetTranscript omits MessageMetadata and NextToken from transcript responses

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTranscript

## Description
Two distinct root causes in connectparticipant GetTranscript's 200 response, both found by replaying the wire corpus (trace set simparity-wire-aws-v1) against the emulator. Smithy model consulted: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/smithy-model/com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda.json (sdkId ConnectParticipant, restJson1). Both causes are response-shape defects: the status and every other member match, so a client only sees members AWS always returns go missing.

Cause 1 of 2: every transcript Item is served without MessageMetadata. AWS: Smithy GetTranscript -> GetTranscriptResponse$Transcript is a list of Item, and Item$MessageMetadata targets MessageMetadata (members MessageId: ChatItemId len 1..256, Receipts: Receipts, MessageProcessingStatus), documented "The metadata related to the message. Currently this supports only information related to message receipts." The recording shows each MESSAGE item of a 200 GetTranscript response carrying MessageMetadata whose MessageId is that item's OWN Id (a value taken from state, not a constant) and whose Receipts is an empty list while no delivered/read receipt has been recorded for it. Emulator: the member is absent from every item - GetTranscript's stored-item projection hard-codes it to nothing (services/.../src/ops/get_transcript.rs, fn transcript_item sets message_metadata: None, with the comment that receipts "belong to the attachment and receipt operations"), and the stored transcript item in src/state.rs models no receipt state at all. Why: delivered/read state is observable ONLY through MessageMetadata.Receipts, so a chat client cannot render delivery ticks and cannot distinguish "no receipts yet" (AWS: Receipts: []) from "metadata unsupported" (emulator: member absent); SDK code that dereferences item.MessageMetadata.MessageId gets a null dereference against the emulator. Fix: GetTranscript's handler (its Item projection) plus the receipt/metadata fields on the stored transcript item in the service state. Traces: connectparticipant-customer-message-read-back-from-transcript#task#7 and connectparticipant-participant-session-lifecycle#task#10.

Cause 2 of 2: the terminal page of GetTranscript omits NextToken entirely. AWS: recorded behavior only - the Smithy model declares GetTranscriptResponse$NextToken (NextToken, len 1..1000) and the @paginated trait (inputToken/outputToken NextToken, pageSize MaxResults) but says nothing about the terminal value; the recorded 200 response carries NextToken PRESENT as the empty string when there is no further page. Emulator: the member is omitted whenever the next page would make no progress (services/.../src/ops/get_transcript.rs: let next_token = (max_results > 0 && page_end < scan.len()).then(...), i.e. None on the last page), so $.NextToken is absent from the body. Why: a client that reads response["NextToken"] directly, or that compares/round-trips the response's member set against AWS's, diverges on every last page - against AWS it reads an empty string, against the emulator the key does not exist. Fix: GetTranscript's handler / output serialization must report the terminal NextToken AWS reports rather than dropping the member. Trace: connectparticipant-participant-session-lifecycle#task#10.

Related open issue: 20261105T033734Z-connectparticipant-7-net-new-causes-connection-token-refusal.md (behavioral) describes the missing MessageMetadata/Receipts state as its Cause 5, but it carries no wire replay key for either fixture step above, so these wire divergences are filed here with their own keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 26 steps, 18 passed, 8 mismatched, score 69.2%.

Cause 1 - fixture connectparticipant-customer-message-read-back-from-transcript step task#7, operation GetTranscript, verdict mismatch, expected status 200 / actual status 200:
  "$.Transcript[0].MessageMetadata: missing (expected {'MessageId': '<the item's own Id>', 'Receipts': []}, got None)"
  (the recorded MessageId is a per-run chat-item id taken from state; the requirement is that the member is present, that MessageId equals the item's own Id and that Receipts is an empty list when no receipt was sent.)

Cause 1 again, plus Cause 2 - fixture connectparticipant-participant-session-lifecycle step task#10, operation GetTranscript, expected status 200 / actual status 200:
  "$.NextToken: missing (expected '', got None)"
  "$.Transcript[0]: missing (expected {'AbsoluteTime': '<shaped-not-asserted>', 'MessageMetadata': {'MessageId': '<shaped-not-asserted>', 'Receipts': []}, 'ContactId': ..., 'Content': 'sim2real wire hello', 'ContentType': 'text/plain', 'DisplayName': ..., 'Id': '<shaped-not-asserted>', 'ParticipantId': '<shaped-not-asserted>', 'ParticipantRole': 'CUSTOMER', 'Type': 'MESSAGE'}, got {... same members WITHOUT MessageMetadata ...})"
  Every other member of the item matched; the only differences are the absent MessageMetadata and the absent NextToken. The contact id, participant id, chat item id and timestamp in the recording are per-run values and are not part of the requirement.

Source of both: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/get_transcript.rs - fn transcript_item (message_metadata: None) and the next_token computation in handle().

## Steps to Reproduce
1. Boot the emulator from this checkout and point a ConnectParticipant client at its data listener.
2. CreateParticipantConnection(ParticipantToken=<token>, Type=["CONNECTION_CREDENTIALS"]) to obtain a ConnectionToken.
3. SendMessage(ConnectionToken=<token>, ContentType="text/plain", Content="sim2real wire hello").
4. GetTranscript(ConnectionToken=<token>) and inspect the raw JSON body:
   - Cause 1: Transcript[0] has no MessageMetadata member (AWS returns MessageMetadata with MessageId = that item's Id and Receipts = []).
   - Cause 2: the body has no NextToken member even though this is the last (only) page (AWS returns NextToken: "").
5. Re-replay fixture steps connectparticipant-customer-message-read-back-from-transcript#task#7 and connectparticipant-participant-session-lifecycle#task#10 to confirm a fix; both must pass.
