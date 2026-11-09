Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-azure-blob-location-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-azure-blob-location-lifecycle
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|CreateLocationAzureBlob|4xx->2xx|$.__type
Issue-Key: datasync|CreateLocationAzureBlob|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|CreateLocationAzureBlob|4xx->2xx|$.errorCode
Issue-Key: datasync|CreateLocationAzureBlob|4xx->2xx|$.message
Issue-Key: datasync|CreateLocationAzureBlob|4xx->2xx|status
Issue-Key: datasync|DescribeLocationAzureBlob|4xx->2xx|$.__type
Issue-Key: datasync|DescribeLocationAzureBlob|4xx->2xx|$.datasyncErrorCode
Issue-Key: datasync|DescribeLocationAzureBlob|4xx->2xx|$.errorCode
Issue-Key: datasync|DescribeLocationAzureBlob|4xx->2xx|$.message
Issue-Key: datasync|DescribeLocationAzureBlob|4xx->2xx|status
Issue-Commit: 60c9edaff1d8a24fe639ae9745da5c1e78ca4ab9

# DataSync Azure Blob location operations accept requests AWS refuses with InvalidRequestException (2 causes)

> _Note: this issue was identified against the code base at commit `60c9edaff1d8a24fe639ae9745da5c1e78ca4ab9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLocationAzureBlob

## Description
Two over-permissive root causes in the Azure Blob location surface: requests that real DataSync refuses with HTTP 400 / InvalidRequestException are answered HTTP 200 by the emulator, with no error envelope at all, so the emulator mints or reports location state that cannot exist in AWS. Smithy model consulted: services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json. Both operations' only modeled client error is InvalidRequestException {message, errorCode, datasyncErrorCode} (client, HTTP 400), so the refusal CLASS is carried by the errorCode/datasyncErrorCode members and the two must agree; the serialized discriminator is $.__type = 'InvalidRequestException'.

=== Cause 1 of 2: CreateLocationAzureBlob does not require credentials when AuthenticationType is SAS ===
AWS: Smithy: CreateLocationAzureBlob input {ContainerUrl @required, AuthenticationType @required -> AzureBlobAuthenticationType enum [SAS, NONE], SasConfiguration (AzureBlobSasConfiguration{Token @required @sensitive, length 1..255}), CmkSecretConfig, CustomSecretConfig}; errors = [InternalException(500, server), InvalidRequestException(400, client)]. The model marks AuthenticationType required but SasConfiguration optional, so the dependency between them is a CROSS-MEMBER business rule the model cannot state. The member doc for SasConfiguration and for CustomSecretConfig show credentials for an Azure Blob location may arrive either inline as a SAS token or by reference to a secret, so real DataSync requires at least one of them when AuthenticationType = SAS and refuses a request carrying neither with HTTP 400, __type 'InvalidRequestException', errorCode = datasyncErrorCode = 'InvalidParameter', and no location created. Message template, fixed text in full (the recording shows no variable slot): "Invalid parameter: SasConfiguration or CustomConfiguration needs to be provided when authentication type is SAS for Azure Blob locations.." — note the DataSync 'Invalid parameter: ' prefix on the sentence and the doubled terminal period, both of which are part of AWS's fixed text for this error class.
Emulator: the request is answered HTTP 200 and a LocationArn is minted. The whole error envelope is absent — replay reports "status: expected 400, got 200" with $.__type, $.errorCode, $.datasyncErrorCode and $.message all missing. The handler validates each member in isolation (shape/pattern/enum) and never evaluates the AuthenticationType -> credential dependency.
Why: a client that relies on AWS to reject a credential-less SAS location at create time instead gets a LocationArn back, persists it, and references it from a task; the misconfiguration AWS surfaces synchronously at create time becomes undetectable, and the negative test for 'SAS selected but no token supplied' cannot be written against the emulator at all.
Fix: cross-member validation in the CreateLocationAzureBlob handler, evaluated before any location state is persisted — when AuthenticationType = SAS and neither SasConfiguration nor CustomSecretConfig (nor CmkSecretConfig) is present, raise InvalidRequestException(400) with errorCode = datasyncErrorCode = 'InvalidParameter' and the message above, and persist nothing.
Traces: datasync-azure-blob-location-lifecycle#task#4.

=== Cause 2 of 2: DescribeLocationAzureBlob answers 200 for a location the account does not hold ===
AWS: Smithy: DescribeLocationAzureBlob input {LocationArn @required, pattern ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):datasync:[a-z\\-0-9]+:[0-9]{12}:location/loc-[0-9a-z]{17}$}; errors = [InternalException(500), InvalidRequestException(400)]. DataSync holds a location only if one was created and not yet deleted — nothing about a location is derivable from the shape of its ARN. A syntactically valid ARN naming no existing location (never created, or created and then DeleteLocation'd) is refused with HTTP 400, __type 'InvalidRequestException', errorCode = datasyncErrorCode = 'LocationNotFound'. Message template: fixed text "Location " + <the LocationArn exactly as supplied in the request> + " is not found." — the ARN is the only variable slot and it comes from the request, not from emulator state.
Emulator: the call returns HTTP 200 with a location body — "status: expected 400, got 200", and $.__type, $.errorCode, $.datasyncErrorCode and $.message all missing. The handler synthesizes the described location from the requested ARN instead of looking it up in service state, so every well-formed location ARN appears to exist and a delete can never be confirmed.
Why: existence checks are the backbone of idempotent provisioning and of delete confirmation. A reconciler that describes a location and creates it only on LocationNotFound never creates anything against the emulator; a client that deletes an Azure Blob location and then describes it to confirm the delete sees it still present, and the 400-bearing not-found path AWS always takes is unreachable.
Fix: the DescribeLocationAzureBlob handler's state lookup — resolve LocationArn in the datasync location store and refuse an absent (or deleted) one with InvalidRequestException(400)/LocationNotFound instead of deriving the response from the request.
Traces: datasync-azure-blob-location-lifecycle#task#6.

Related open issues, neither of which covers these causes: 20261029T180920Z-datasync-accepts-three-requests-aws-refuses-unassumable-bucket.md files the analogous not-found gap for DescribeLocationS3 and cross-member gaps for CreateLocationS3/CreateTask, but carries no Azure Blob fixture step; 20261029T180920Z-datasync-refusals-omit-errorcode-datasyncerrorcode-and-use-emulator.md files refusals the emulator DOES make but mis-serializes, whereas here no refusal is made at all. 20261029T172420Z-datasync-createlocationefs-stub.md and -createlocationobjectstorage-stub.md are unimplemented-operation stubs for other location types; CreateLocationAzureBlob and DescribeLocationAzureBlob are implemented and answering 200.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200). Two verdict=mismatch clusters with empty stub_path and empty owned_by, both expected_status 400 / actual_status 200 — the emulator accepted a request AWS refuses.

1) Cause 1 — fixture datasync-azure-blob-location-lifecycle, step task#4, operation CreateLocationAzureBlob (affected_count 1):
   status: expected 400, got 200
   $.__type: missing (expected 'InvalidRequestException', got None)
   $.errorCode: missing (expected 'InvalidParameter', got None)
   $.datasyncErrorCode: missing (expected 'InvalidParameter', got None)
   $.message: missing (expected 'Invalid parameter: SasConfiguration or CustomConfiguration needs to be provided when authentication type is SAS for Azure Blob locations..', got None)
   Message anatomy: the entire sentence is FIXED text for this error class (including the 'Invalid parameter: ' prefix and the doubled final period); there is no request- or state-derived slot.
   divergence_key: service=datasync|CreateLocationAzureBlob|status=400->200|keys=$.__type,$.datasyncErrorCode,$.errorCode,$.message,status

2) Cause 2 — fixture datasync-azure-blob-location-lifecycle, step task#6, operation DescribeLocationAzureBlob (affected_count 1):
   status: expected 400, got 200
   $.__type: missing (expected 'InvalidRequestException', got None)
   $.errorCode: missing (expected 'LocationNotFound', got None)
   $.datasyncErrorCode: missing (expected 'LocationNotFound', got None)
   $.message: missing (expected 'Location arn:aws:datasync:us-west-2:000000000000:location/loc-00000000000000001 is not found.', got None)
   Message anatomy: FIXED text 'Location ' ... ' is not found.'; the single variable slot is the LocationArn echoed from the request. The account id, region and loc- id in the recorded string are that run's values and are NOT part of the requirement.
   divergence_key: service=datasync|DescribeLocationAzureBlob|status=400->200|keys=$.__type,$.datasyncErrorCode,$.errorCode,$.message,status

Model references (services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json): CreateLocationAzureBlob input {ContainerUrl @required, AuthenticationType @required (enum SAS|NONE), SasConfiguration, CmkSecretConfig, CustomSecretConfig}; AzureBlobSasConfiguration.Token @required @sensitive length 1..255; DescribeLocationAzureBlob.LocationArn @required with the loc-[0-9a-z]{17} ARN pattern; InvalidRequestException{message, errorCode, datasyncErrorCode}, client, HTTP 400 for both operations.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 datasync fixtures.

Cause 1: call CreateLocationAzureBlob with a valid ContainerUrl and AuthenticationType = 'SAS' but NO SasConfiguration, CmkSecretConfig or CustomSecretConfig. Observe HTTP 200 with a minted LocationArn instead of HTTP 400 / __type InvalidRequestException with errorCode = datasyncErrorCode = 'InvalidParameter' and the 'Invalid parameter: SasConfiguration or CustomConfiguration needs to be provided when authentication type is SAS for Azure Blob locations..' message. Replay: datasync-azure-blob-location-lifecycle#task#4

Cause 2: create an Azure Blob location, DeleteLocation it (or simply take any well-formed location ARN that was never created), then call DescribeLocationAzureBlob with that ARN. Observe a HTTP 200 location body instead of HTTP 400 / __type InvalidRequestException with errorCode = datasyncErrorCode = 'LocationNotFound' and message 'Location <requested LocationArn> is not found.'. Replay: datasync-azure-blob-location-lifecycle#task#6
