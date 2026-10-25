Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.channel-lifecycle-create-active-delete-404
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.channel-lifecycle-create-active-delete-404|run#25|AssertionError|assert malformed_dest_code == "InvalidParameterException", (
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# CreateChannel misclassifies a non-ARN Destination Location as EventDataStoreARNInvalidException instead of InvalidParameterException

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
REOPENS 20261022T230533Z-cloudtrail-createchannel-stub.md: its fix left the call refused. That issue implemented `CreateChannel` only for the destination class its recorded wire step exercised (an ARN-shaped `Location` whose resource type is wrong -> `EventDataStoreARNInvalidException`), and the implementation now routes EVERY `Location` that is not a well-formed event data store ARN into that same refusal. The behavioral check that exercises the other destination class is still refused, with the wrong error code and the wrong message.

Cause 1 of 1: `CreateChannel` collapses two distinct `Destination$Location` rejection classes into one.

AWS: `CreateChannel` models BOTH `InvalidParameterException` (HTTP 400, awsQueryError code `InvalidParameter`, doc "The request includes a parameter that is not valid.") and `EventDataStoreARNInvalidException` (HTTP 400, awsQueryError code `EventDataStoreARNInvalid`, doc "The specified event data store ARN is not valid or does not map to an event data store in your account.") as separate errors (Smithy: `CreateChannel` errors list; `Destination$Location` is `@required`, length 3..1024, pattern `^[a-zA-Z0-9._/\-:*]+$`). The model does not say which class each malformed value falls in, so this is recorded behavior: for a `Destination` of `Type` `EVENT_DATA_STORE` whose `Location` is a BARE, non-ARN, non-ID value, CloudTrail answers HTTP 400 `InvalidParameterException` with the message "The EventDataStore value must be a valid EventDataStore ARN or ID." — that message is entirely FIXED text with NO request-derived part (it names neither the value sent nor the channel name; AWS deliberately does not echo it, and the wording shows AWS also admits a bare event data store ID in `Location`, not only an ARN). `EventDataStoreARNInvalidException` is reserved for a value that IS written as an ARN but is not a valid event data store ARN, whose message is "The event data store ARN <location> format is not valid." with `<location>` taken verbatim from the request (that wording is the one the already-fixed stub issue's recorded step pinned). A well-formed event data store ARN naming no store is a third class, `EventDataStoreNotFoundException`, message "The event data store <location> was not found." with `<location>` from the request.

Emulator: `CreateChannel` answers HTTP 400 `EventDataStoreARNInvalidException` for the bare non-ARN `Location`, where AWS answers HTTP 400 `InvalidParameterException` with "The EventDataStore value must be a valid EventDataStore ARN or ID.". The replay fails at the check's assertion `expected InvalidParameterException, got EventDataStoreARNInvalidException`. The `EventDataStoreNotFoundException` leg of the same check is never reached, so it is unverified behind this refusal. The cause is a single catch-all branch: the destination resolver reads the `Location` through an ARN-only grammar and maps every parse failure — ARN-shaped or not — to one rejection variant, so the non-ARN class has no path to `InvalidParameterException` at all even though the handler already has a branch that can emit it (it is used only for a wrong `Destination$Type`).

Why: a client that branches on the error code to tell "you typed something that is not an identifier" from "you typed an ARN but it is malformed" (and from "that store does not exist") takes the wrong branch and reports the wrong remediation to its user. The message is also wrong, so a client or test asserting on AWS's documented refusal text — the fixed sentence "The EventDataStore value must be a valid EventDataStore ARN or ID." — sees unrelated prose that echoes the request value. Because the emulator's grammar accepts ONLY the ARN form, a caller passing the bare event data store ID that AWS's own message says is acceptable is also refused, rather than resolved.

Fix: the destination-rejection classifier in the CloudTrail channel state module, and the error mapping in the `CreateChannel` handler that consumes it. It must split "not an identifier at all" (bare non-ARN, non-ID value -> `InvalidParameterException` with AWS's fixed message) from "ARN-shaped but not a valid event data store ARN" (-> `EventDataStoreARNInvalidException`, message echoing the request `Location`), keeping the existing `EventDataStoreNotFoundException` and `InactiveEventDataStoreException` classes unchanged. The same classifier is shared by every operation that takes a `Destinations` list, so `UpdateChannel` must move with it.

Traces: cloudtrail.channel-lifecycle-create-active-delete-404 (assertion at check_cloudtrail_channel_lifecycle_create_active_delete_404.py line 85). Code: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/state/channels.rs lines 486-530 (`resolve_channel_destination`; the catch-all `DestinationRejection::ArnInvalid` at lines 505-509 and the ARN-only reader `parse_destination_event_data_store_arn` at lines 116-130), and services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/create_channel.rs lines 241-245 and 322-364 (`destination_error`).

## Evidence
Failing check: [mismatch] cloudtrail.channel-lifecycle-create-active-delete-404

Replay output:
  File "check_cloudtrail_channel_lifecycle_create_active_delete_404.py", line 85, in run
    assert malformed_dest_code == "InvalidParameterException", (
  AssertionError: expected InvalidParameterException, got EventDataStoreARNInvalidException

Request: CreateChannel with Name=<unique channel name>, Source="Custom", Destinations=[{Type: "EVENT_DATA_STORE", Location: "not-an-eds-arn"}] (a bare, non-ARN, non-ID value).

Expected (real AWS, us-west-2 recording that the check's oracle was blessed from):
  HTTP 400
  Error.Code = InvalidParameterException
  Error.Message = "The EventDataStore value must be a valid EventDataStore ARN or ID."
    -> the whole sentence is FIXED text; no part of it comes from the request or from state.

Actual (emulator):
  HTTP 400
  Error.Code = EventDataStoreARNInvalidException
  Error.Message = the "The event data store ARN <location> format is not valid." wording,
    i.e. the ARN-malformed template, with <location> echoed from the request.

Contrast class that IS correct today, from the recorded step in the already-fixed stub issue
20261022T230533Z-cloudtrail-createchannel-stub.md: an ARN-shaped Location
(arn:aws:cloudtrail:<region>:<account>:eventdatastorewrong/<uuid>) is expected to answer 400
EventDataStoreARNInvalidException with "The event data store ARN <location> format is not valid."
— so the emulator holds exactly one of the two classes and applies it to both.

Smithy model (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json):
  CreateChannel errors include InvalidParameterException (httpError 400, awsQueryError InvalidParameter),
  EventDataStoreARNInvalidException (httpError 400, awsQueryError EventDataStoreARNInvalid) and
  EventDataStoreNotFoundException (httpError 404; the emulator already answers 400 for it per its
  recorded wire step) as DISTINCT errors.
  Destination$Type: required, enum DestinationType (EVENT_DATA_STORE | AWS_SERVICE).
  Destination$Location: required, length 3..1024, pattern ^[a-zA-Z0-9._/\-:*]+$ — the pattern admits
  "not-an-eds-arn", so boundary validation passes and the handler owns the refusal.

Code path that produces the divergence:
  services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/state/channels.rs:505-509
    let Some((arn_region, id)) = parse_destination_event_data_store_arn(location) else {
        return Err(DestinationRejection::ArnInvalid(
            super::event_data_store_arn_invalid_message(location, request_region),
        ));
    };
  -> every non-ARN Location takes this branch; DestinationRejection::InvalidParameter is only ever
     constructed for a wrong Destination$Type (same file, lines 492-500).
  services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/create_channel.rs:328-334
    maps DestinationRejection::ArnInvalid -> EventDataStoreARNInvalidException.

Legs of the same check left unverified behind this refusal: CreateChannel with a well-formed but
nonexistent event data store ARN (expected EventDataStoreNotFoundException, message
"The event data store <arn> was not found." with <arn> from the request), and the follow-up
ListChannels assertion that neither rejected CreateChannel created a channel.

Passing legs of the same check (for scope): GetChannel on a malformed ARN already answers
ChannelARNInvalidException "The format of the channel ARN <value> is not valid.", and
GetChannel/UpdateChannel/DeleteChannel on a well-formed-but-absent channel ARN already answer
ChannelNotFoundException "The channel <arn> was not found." — those are correct and must not regress.

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudtrail client at it (region us-west-2).
2. Call CreateChannel with a destination whose Location is a bare, non-ARN, non-ID string:
     ct.create_channel(
         Name="sim2real-channel-example",
         Source="Custom",
         Destinations=[{"Type": "EVENT_DATA_STORE", "Location": "not-an-eds-arn"}],
     )
3. Observe the refusal: the emulator answers HTTP 400 with Error.Code
   EventDataStoreARNInvalidException and the "event data store ARN ... format is not valid"
   message. Real AWS answers HTTP 400 with Error.Code InvalidParameterException and the fixed
   message "The EventDataStore value must be a valid EventDataStore ARN or ID.".
4. Contrast with the ARN-shaped class, which must keep its current answer: call CreateChannel with
   Location="arn:aws:cloudtrail:us-west-2:<account>:eventdatastorewrong/00000000-0000-0000-0000-000000000000"
   and observe 400 EventDataStoreARNInvalidException with "The event data store ARN <location>
   format is not valid." — the same answer the emulator currently gives for step 2.
5. Then call CreateChannel with a well-formed but nonexistent store ARN,
   "arn:aws:cloudtrail:us-west-2:<account>:eventdatastore/00000000-0000-0000-0000-000000000000",
   and confirm 400 EventDataStoreNotFoundException with "The event data store <arn> was not found."
   (the leg the current misclassification hides), then ListChannels and confirm no channel named in
   steps 2-5 was created.
6. Re-replay cloudtrail.channel-lifecycle-create-active-delete-404 to confirm the fix.
