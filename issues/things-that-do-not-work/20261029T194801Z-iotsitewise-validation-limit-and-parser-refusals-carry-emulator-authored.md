Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.tag-resource-too-many-tags-rejected iotsitewise.too-many-tags-rejected iotsitewise.delete-time-series-missing-identifier-rejected iotsitewise.execute-query-malformed-sql-rejected iotsitewise.gateway-duplicate-core-device-rejected
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.tag-resource-too-many-tags-rejected|run#11|AssertionError|assert message == "Only 50 user tags are allowed on the resource", f"unexpected message: {message!r}"
Issue-Key: iotsitewise.too-many-tags-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotsitewise.delete-time-series-missing-identifier-rejected|run#9|AssertionError|assert message == "Please provide either alias or assetId and propertyId", f"unexpected message: {message!r}"
Issue-Key: iotsitewise.execute-query-malformed-sql-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotsitewise.gateway-duplicate-core-device-rejected|run#9|AssertionError|assert err.get("Message") == expected_message, err
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# Validation, limit and parser refusals carry emulator-authored messages (TagResource, DeleteTimeSeries, ExecuteQuery, CreateGateway)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Five refusal paths land on the correct HTTP status and error code but answer a message the emulator wrote itself; one of them leaks a third-party parser's error text to the client. This issue bundles 5 causes; all five must be closed.

Cause 1 of 5: the resource-level tag limit message is the emulator's own sentence and interpolates a count.
AWS: Smithy: TagResource declares LimitExceededException (httpError 410) and TooManyTagsException (httpError 400), each with a `message` member. When the target resource already carries the maximum of 50 tags and a further key is requested, AWS refuses with LimitExceededException and the message "Only 50 user tags are allowed on the resource" — entirely fixed text (the limit 50 is the service's own constant, not a value echoed from the request), with NO variable parts.
Emulator: answers the right code (LimitExceededException) with "The request would leave <n> tags on the resource; a resource holds at most 50 tags.", a sentence AWS does not produce, carrying a computed resulting-tag count.
Why: a client distinguishing "too many tags on this resource" from other 410s by message fails to match, and the computed count is not part of the contract.
Fix: the TagResource handler's resource-level tag-count precondition must emit AWS's fixed sentence.
Traces: iotsitewise.tag-resource-too-many-tags-rejected.

Cause 2 of 5: the map-length constraint-violation message interpolates a "'<n> entries'" value AWS's text does not carry.
AWS: Smithy: TagResource input member tags targets TagMap with @required and @length min 1 max 50. A single call supplying 51 entries is a request-shape violation, refused 400 InvalidRequestException with AWS's standard constraint text, which for a COLLECTION member omits the value clause entirely: "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must have length less than or equal to "; variable parts: the member name from the request and the modeled @length max.
Emulator: emits the same sentence with a value clause spliced in — "1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50" — i.e. the shared constraint renderer always prints a Value '<x>' clause and invents the rendering "<n> entries" for maps and lists.
Why: the validation message is part of the contract; assertions on it fail, and the invented "<n> entries" rendering misreports how the service describes a collection-length violation for every length-constrained map/list member in the crate, not just this one.
Fix: the shared request-validation constraint-message renderer must omit the Value '<x>' clause for length violations on collection (map/list) members, keeping it only where AWS quotes a scalar value.
Traces: iotsitewise.too-many-tags-rejected.

Cause 3 of 5: DeleteTimeSeries' missing-identifier message is reworded and repunctuated.
AWS: Smithy: DeleteTimeSeries declares InvalidRequestException (httpError 400, member message); alias, assetId and propertyId are all OPTIONAL httpQuery members, so the "supply alias, or assetId+propertyId" rule is enforced by the handler, not the shape. A call with none of them is refused 400 with "Please provide either alias or assetId and propertyId" — entirely fixed text, no variable parts, no trailing period.
Emulator: answers the right code with "Specify either alias, or assetId and propertyId." — different verb, an extra comma and a trailing period.
Why: clients and tests matching the documented text fail; the emulator's paraphrase is not the contract.
Fix: the DeleteTimeSeries handler's identifier-presence validation must emit AWS's sentence verbatim.
Traces: iotsitewise.delete-time-series-missing-identifier-rejected.

Cause 4 of 5: ExecuteQuery leaks the underlying SQL parser's error text instead of the service's SiteWiseQL message.
AWS: Smithy: ExecuteQuery declares InvalidRequestException (httpError 400, member message) and ValidationException (400); input queryStatement is @required, @length 1..10000, @sensitive. A syntactically invalid statement is refused 400 InvalidRequestException with "Invalid SiteWiseQL Query statement: no viable alternative at input '<offending input>'" — fixed text "Invalid SiteWiseQL Query statement: no viable alternative at input '" and the closing quote; variable part: the offending input span taken from the request's queryStatement (in the recording, the whole statement).
Emulator: answers 400 with "Invalid query statement: sql parser error: Expected: an SQL statement, found: <token> at Line: <n>, Column: <n>" — the embedded Rust SQL-parser crate's diagnostic, surfaced verbatim, naming neither SiteWiseQL nor AWS's diagnosis.
Why: the message exposes an emulator implementation detail (a parser crate's wording, line/column scheme and token naming) as if it were service output; a client matching the documented SiteWiseQL text fails, and operators are pointed at a parser the service does not use. It also risks echoing a @sensitive statement in a non-AWS shape.
Fix: the ExecuteQuery handler's statement-parse failure must be translated into the service's InvalidRequestException message, quoting the offending input, rather than formatting the parser error through.
Traces: iotsitewise.execute-query-malformed-sql-rejected.

Cause 5 of 5: CreateGateway's duplicate-coreDeviceThingName refusal names the request member instead of the Core Device.
AWS: Smithy: CreateGateway declares InvalidRequestException (httpError 400, member message) and ResourceAlreadyExistsException (409); gatewayPlatform is @required and carries greengrassV2.coreDeviceThingName. Creating a second gateway for a coreDeviceThingName already claimed by another gateway is refused 400 InvalidRequestException with "Core Device/IoT Core Thing: <coreDeviceThingName> is already associated with a SiteWise Gateway." — fixed text "Core Device/IoT Core Thing: " and " is already associated with a SiteWise Gateway."; variable part: the coreDeviceThingName from the request body.
Emulator: answers the right code with "The greengrassV2.coreDeviceThingName <name> is already registered to another gateway." — the emulator's own sentence, addressing the wire member path rather than the Core Device/IoT Core Thing the service names.
Why: a client matching the documented refusal text fails, and the message describes the conflict in terms of a request field rather than the externally-owned IoT Core thing that actually holds the claim.
Fix: the CreateGateway handler's duplicate-core-device check must emit the service's Core Device message.
Traces: iotsitewise.gateway-duplicate-core-device-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the emulator built from mainline: 46/60 passed, all 14 failures in iotsitewise. Every failure below matches on status and error code and diverges only on the message text. Quoted counts, tokens and names are that run's values filling the templates; the fixed text and variable parts are stated per cause above.

1) iotsitewise.tag-resource-too-many-tags-rejected — TagResource with one extra key against an asset-model ARN already carrying 50 tags; code LimitExceededException as expected, message expected 'Only 50 user tags are allowed on the resource', got 'The request would leave 51 tags on the resource; a resource holds at most 50 tags.'

2) iotsitewise.too-many-tags-rejected — TagResource with 51 tags in one call; code InvalidRequestException as expected, message expected "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", got "1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50".

3) iotsitewise.delete-time-series-missing-identifier-rejected — DeleteTimeSeries with no alias/assetId/propertyId; code InvalidRequestException as expected, message expected 'Please provide either alias or assetId and propertyId', got 'Specify either alias, or assetId and propertyId.'

4) iotsitewise.execute-query-malformed-sql-rejected — ExecuteQuery(queryStatement='SELEKT * FRUM asset WHERE'); code InvalidRequestException as expected, message expected "Invalid SiteWiseQL Query statement: no viable alternative at input 'SELEKT * FRUM asset WHERE'", got 'Invalid query statement: sql parser error: Expected: an SQL statement, found: SELEKT at Line: 1, Column: 1'.

5) iotsitewise.gateway-duplicate-core-device-rejected — CreateGateway reusing a coreDeviceThingName already held by another gateway; code InvalidRequestException as expected, message expected 'Core Device/IoT Core Thing: <coreDeviceThingName> is already associated with a SiteWise Gateway.', got 'The greengrassV2.coreDeviceThingName <coreDeviceThingName> is already registered to another gateway.'

Model grounding: TagResource.tags targets TagMap with @required and @length 1..50, and TagResource declares LimitExceededException (410), InvalidRequestException (400) and TooManyTagsException (400); DeleteTimeSeries declares InvalidRequestException (400) with alias/assetId/propertyId all optional httpQuery members; ExecuteQuery declares InvalidRequestException (400) and ValidationException (400) with @sensitive queryStatement; CreateGateway declares InvalidRequestException (400) and ResourceAlreadyExistsException (409).

## Steps to Reproduce
1. Re-replay iotsitewise.tag-resource-too-many-tags-rejected: CreateAssetModel, TagResource with 50 keys on its ARN, then TagResource with one more key — observe the 410 message is the emulator's 'would leave <n> tags' sentence.
2. Re-replay iotsitewise.too-many-tags-rejected: TagResource with 51 entries in a single call — observe the 400 constraint message splices in "Value '51 entries'".
3. Re-replay iotsitewise.delete-time-series-missing-identifier-rejected: POST /timeseries/delete with no alias, assetId or propertyId — observe the 400 message is 'Specify either alias, or assetId and propertyId.'
4. Re-replay iotsitewise.execute-query-malformed-sql-rejected: POST /queries/execution with queryStatement='SELEKT * FRUM asset WHERE' — observe the 400 message is the Rust sql-parser diagnostic.
5. Re-replay iotsitewise.gateway-duplicate-core-device-rejected: CreateGateway with a greengrassV2 coreDeviceThingName, then CreateGateway again with the same thing name — observe the 400 message names 'greengrassV2.coreDeviceThingName' instead of 'Core Device/IoT Core Thing'.
