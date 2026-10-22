Issue-Source: trace-sweep-divergence
Issue-Trace: fsx-create-file-system-validation-precedence
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fsx-create-file-system-validation-precedence
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx|CreateFileSystem|4xx->2xx|$.InvalidSubnetId
Issue-Key: fsx|CreateFileSystem|4xx->2xx|$.Message
Issue-Key: fsx|CreateFileSystem|4xx->2xx|$.__type
Issue-Key: fsx|CreateFileSystem|4xx->2xx|status
Issue-Commit: f2a9c91c50f7e55687472cf5b456a197c6c3ca83

# FSx CreateFileSystem skips request validation and returns 200 instead of a 400 error

> _Note: this issue was identified against the code base at commit `f2a9c91c50f7e55687472cf5b456a197c6c3ca83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
The emulator's CreateFileSystem handler creates and returns a file system for requests that AWS FSx rejects before any resource is created. Two distinct pre-creation guards are missing; both end with HTTP 200 and a FileSystem payload where AWS answers HTTP 400 with a modeled error. 2 causes.

Cause 1 of 2: subnet existence is never checked (fixture fsx-create-file-system-validation-precedence, step task#2; same cause also at task#3).
AWS: CreateFileSystem validates that every id in the required SubnetIds list names an existing subnet in the account/VPC. When one does not, it fails with the modeled error InvalidNetworkSettings at HTTP 400 and populates the member that identifies the offending id. Smithy: CreateFileSystem errors include InvalidNetworkSettings (client, httpError 400); InvalidNetworkSettings members are Message (ErrorMessage, length 1..256), InvalidSubnetId (SubnetId, pattern ^(subnet-[0-9a-f]{8,})$), InvalidSecurityGroupId, InvalidRouteTableId; doc "One or more network settings specified in the request are invalid." The recorded response carries __type InvalidNetworkSettings, InvalidSubnetId set to the requested subnet id, and a Message of the form "The subnet ID '<subnet-id>' does not exist".
Emulator: returns HTTP 200 with a created file system. __type, InvalidSubnetId and Message are all absent.
Why: a client that provisions into a non-existent or wrong-VPC subnet gets a success and a file system id back. Error handling, retry and the operator-facing "which subnet was wrong" signal (InvalidSubnetId) never fire, and the caller's view of the account now contains a file system that AWS would never have created, so every subsequent step of its workflow is built on state real FSx does not have.
Fix: CreateFileSystem's validation/handler in the fsx service crate - resolve each SubnetIds entry against known subnets before allocating the file system, and raise InvalidNetworkSettings (400) with InvalidSubnetId set to the failing id.

Cause 2 of 2: a missing client request token is accepted (fixture fsx-create-file-system-validation-precedence, step task#4).
AWS: recorded behavior only - the Smithy model marks CreateFileSystem.ClientRequestToken as @idempotencyToken (ClientRequestToken, length 1..63, pattern ^[A-za-z0-9_.-]{0,63}$) and not @required, so the model alone does not state this. The recording shows that a CreateFileSystem request that omits the token is rejected at HTTP 400 with __type BadRequest (Smithy: BadRequest, client, httpError 400, member Message; doc "A generic error indicating a failure with a client request.") and a Message of the form "A client request token must be specified."
Emulator: returns HTTP 200 with a created file system; neither __type nor Message is present.
Why: the token is what makes CreateFileSystem idempotent. Accepting a tokenless request means the emulator will happily create a second file system on a retry that AWS would have refused outright, so clients cannot exercise or regression-test their idempotency handling against the emulator.
Fix: CreateFileSystem's validation in the fsx service crate - require ClientRequestToken on the request path and return BadRequest (400) with a Message naming the missing token when it is absent, before any state is mutated.

Traces: fsx-create-file-system-validation-precedence#task#2 (and #task#3), fsx-create-file-system-validation-precedence#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (27 steps, 0 passed; state_reset_failures 0).

Cause 1 - fsx-create-file-system-validation-precedence#task#2 (cluster also covers task#3), CreateFileSystem:
  expected status 400, actual 200
  $.__type: missing (expected 'InvalidNetworkSettings', got None)
  $.InvalidSubnetId: missing (expected the requested subnet id, got None)
  $.Message: missing (expected "The subnet ID '<subnet-id>' does not exist", got None)

Cause 2 - fsx-create-file-system-validation-precedence#task#4, CreateFileSystem:
  expected status 400, actual 200
  $.__type: missing (expected 'BadRequest', got None)
  $.Message: missing (expected 'A client request token must be specified.', got None)

Model evidence: smithy_query operation=CreateFileSystem on services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/smithy-model/com-amazonaws-fsx-awssimbaapiservice-v20180301.json - errors include BadRequest and InvalidNetworkSettings, both client/400; SubnetIds is @required; ClientRequestToken is @idempotencyToken.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an FSx client at its data listener.
2. Cause 1: call CreateFileSystem with a valid FileSystemType/StorageCapacity and a SubnetIds list containing a subnet id that does not exist. Expected: HTTP 400, __type InvalidNetworkSettings, InvalidSubnetId equal to that subnet id. Actual: HTTP 200 with a FileSystem payload. (Replay fsx-create-file-system-validation-precedence step task#2; task#3 is the same cause.)
3. Cause 2: call CreateFileSystem with an otherwise valid body but no ClientRequestToken member. Expected: HTTP 400, __type BadRequest, Message naming the missing client request token. Actual: HTTP 200 with a FileSystem payload. (Replay fsx-create-file-system-validation-precedence step task#4.)
