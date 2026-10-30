Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-object-storage-location-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-object-storage-location-lifecycle
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|DescribeLocationObjectStorage|4xx->2xx|$.__type
Issue-Key: datasync|DescribeLocationObjectStorage|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|DescribeLocationObjectStorage|4xx->2xx|$.errorCode
Issue-Key: datasync|DescribeLocationObjectStorage|4xx->2xx|$.message
Issue-Key: datasync|DescribeLocationObjectStorage|4xx->2xx|status
Issue-Commit: 16037d1b3f3ebb553cc9a0c9d57f9342f591b149

# DataSync DescribeLocationObjectStorage returns 200 for a location that does not exist

> _Note: this issue was identified against the code base at commit `16037d1b3f3ebb553cc9a0c9d57f9342f591b149`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLocationObjectStorage

## Description
One net-new cause: the object-storage location read is answered from the request instead of from service state, so a location the account does not hold appears to exist.

Cause 1 of 1: DescribeLocationObjectStorage does not check that the location exists.

AWS: DescribeLocationObjectStorage takes a single @required LocationArn (Smithy: DescribeLocationObjectStorageRequest.LocationArn @required, @length max 128, pattern ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):datasync:[a-z\-0-9]+:[0-9]{12}:location/loc-[0-9a-z]{17}$). Its only modeled client error is InvalidRequestException (client, HTTP 400, members message/errorCode/datasyncErrorCode), so the refusal class is carried by those two code members rather than by a distinct error shape. A well-formed location ARN that names no location held by the account is refused with HTTP 400, __type 'InvalidRequestException', errorCode = datasyncErrorCode = 'LocationNotFound', and a message whose fixed text is "Location " + <the LocationArn exactly as supplied in the request> + " is not found." — the ARN is the only variable slot; the rest of the sentence is fixed. DataSync holds a location only because one was created: nothing about a location's existence is derivable from the shape of its ARN. Status and error code are taken from the recorded response for this operation.

Emulator: the request is answered HTTP 200 with a location body. The replay reports "status: expected 400, got 200" and the entire error envelope absent — $.__type missing (expected 'InvalidRequestException'), $.errorCode and $.datasyncErrorCode missing (expected 'LocationNotFound'), $.message missing. The handler synthesizes an object-storage location response from the ARN in the request rather than looking it up, so every syntactically valid location ARN reads back as an existing location.

Why: existence checks are how clients drive idempotent provisioning and confirm deletes. A reconciler that describes a location and creates it only on the not-found refusal never creates anything against the emulator; a client that deletes a location and then describes it to confirm the delete sees it still present; and negative tests for the LocationNotFound path cannot be written at all. Because the emulator does refuse some other not-found reads, the behavior looks ARN-dependent and inconsistent.

Fix: the DescribeLocationObjectStorage handler and its state lookup — resolve LocationArn in the datasync service state and, when no such location is held, raise InvalidRequestException with HTTP 400, __type InvalidRequestException, errorCode = datasyncErrorCode = 'LocationNotFound' and the "Location <arn> is not found." message, instead of deriving a 200 body from the request.

Related open issue, not a duplicate: 20261029T180920Z-datasync-accepts-three-requests-aws-refuses-unassumable-bucket.md describes the same missing-existence-check pattern for DescribeLocationS3 (its Cause 2); it does not carry this fixture step's replay key and names a different operation's handler, so this object-storage divergence is filed here and that issue is named for the fixer's benefit.

Traces: Cause 1 — datasync-object-storage-location-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false). One verdict=mismatch cluster, not owned by any open issue and not blocked by a stub:

datasync-object-storage-location-lifecycle#task#6, DescribeLocationObjectStorage (affected_count 1):
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidRequestException', got None)
  $.datasyncErrorCode: missing (expected 'LocationNotFound', got None)
  $.errorCode: missing (expected 'LocationNotFound', got None)
  $.message: missing (expected 'Location <location-arn> is not found.', got None)

Message template breakdown: fixed text is "Location " and " is not found."; the single variable slot is the LocationArn taken verbatim from the request (the recording's concrete value is one fill of that template, not part of the requirement).

Model references (services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json): DescribeLocationObjectStorage input {LocationArn @required, @length 0..128, pattern ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):datasync:[a-z\-0-9]+:[0-9]{12}:location/loc-[0-9a-z]{17}$}; errors = [InternalException (server, 500), InvalidRequestException (client, 400)]; InvalidRequestException members = {message, errorCode, datasyncErrorCode}.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 datasync fixtures.

Cause 1: issue DescribeLocationObjectStorage with a well-formed DataSync location ARN that was never created in this emulator (or create an object-storage location with CreateLocationObjectStorage, DeleteLocation it, then describe it). Observe HTTP 200 with a synthesized location body instead of HTTP 400 / __type InvalidRequestException with errorCode = datasyncErrorCode = 'LocationNotFound' and message "Location <the requested ARN> is not found.". Replay: datasync-object-storage-location-lifecycle#task#6
