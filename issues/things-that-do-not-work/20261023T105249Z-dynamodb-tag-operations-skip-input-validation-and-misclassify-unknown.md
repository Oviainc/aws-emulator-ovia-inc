Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-resource-tag-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-resource-tag-lifecycle dynamodb-tagging-and-resource-policy dynamodb-table-tagging-lifecycle
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TagResource|4xx->2xx|$
Issue-Key: dynamodb|TagResource|4xx->2xx|status
Issue-Key: dynamodb|ListTagsOfResource|4xx->4xx|$.__type
Issue-Key: dynamodb|ListTagsOfResource|4xx->4xx|$.message
Issue-Key: dynamodb|TagResource|4xx->4xx|$.message
Issue-Key: dynamodb|UntagResource|4xx->4xx|$.message
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Tag operations skip input validation and misclassify unknown-resource errors

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The tagging surface accepts input AWS rejects, and when it does reject it picks the wrong error class and reports a resource identifier the caller never sent. 5 causes, all dynamodb.

Cause 1 of 5: TagResource accepts a Tags list containing duplicate keys and answers 200.
AWS: Smithy: TagResource input Tags is a required TagList; output is Unit (empty 200). The recording shows AWS rejecting a request whose Tags list repeats a key with HTTP 400 and __type 'com.amazon.coral.validate#ValidationException' (recorded message, as an example: 'Duplicate Tag Keys provided as input: Duplicate Tag Key found env'); the rule is that a tag key may appear at most once per request.
Emulator: HTTP 200 with an empty body - the duplicate is accepted.
Why: the request silently succeeds and one of the two values wins arbitrarily, so a client that relies on AWS rejecting a malformed tag set (for example one assembled from two merged config sources) ships a table whose tags differ from what it believes it set, and its error-path tests never fire.
Fix: TagResource's input validation - reject a Tags list with a repeated Key as a 400 ValidationException before touching tag state.
Traces: dynamodb-resource-tag-lifecycle#task#7.

Cause 2 of 5: ListTagsOfResource on a malformed ResourceArn returns ResourceNotFoundException instead of ValidationException.
AWS: Smithy: ListTagsOfResource input ResourceArn is a required ResourceArnString (@length 1..1283) and the operation's modeled client error is ResourceNotFoundException (400). The recording shows that an ARN that is not a well-formed DynamoDB table ARN is an input-validation failure, not a lookup miss: HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' (recorded message, as an example: 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:dynamodb:us-east-1:000000000000:table/bad').
Emulator: HTTP 400 but __type 'ResourceNotFoundException' with 'Requested resource not found: Table: bad not found' - it parses the bad ARN as a table name and reports a miss.
Why: the error code is the part clients branch on. Treating a malformed ARN as "not found" makes a retry/create-if-absent path trigger on input the caller should have been told to fix, and masks client bugs that AWS surfaces as validation failures.
Fix: the ListTagsOfResource handler's ResourceArn validation - parse and validate the ARN (partition/service/region/account/resource-type) and raise ValidationException on a malformed value, before the table lookup.
Traces: dynamodb-tagging-and-resource-policy#task#16.

Cause 3 of 5: TagResource's not-found error names the table instead of the ResourceArn the caller sent.
AWS: Smithy: TagResource input is ResourceArn (required, ResourceArnString) and its modeled client error is ResourceNotFoundException (400). The message is built from the request: for an ARN-addressed operation AWS reports the identifier it was given - 'Requested resource not found: ResourceArn: <the ARN from the request> not found'. Status 400 and the error code match.
Emulator: substitutes a different identifier - 'Requested resource not found: Table: <table-name> not found' - reporting a member (table name) the caller never supplied.
Why: the message is the only place the failing input is echoed back; echoing a derived table name instead of the submitted ResourceArn breaks log correlation and makes it impossible to tell which of several ARNs in a batch of tag calls was rejected.
Fix: the TagResource handler's error construction - render ResourceNotFoundException from the request's ResourceArn member for ARN-addressed tag operations.
Traces: dynamodb-table-tagging-lifecycle#task#8.

Cause 4 of 5: ListTagsOfResource's not-found error has the same wrong identifier.
AWS: as cause 3 - 'Requested resource not found: ResourceArn: <ARN from the request> not found', built from the required ResourceArn input member.
Emulator: 'Requested resource not found: Table: <table-name> not found'.
Why: as cause 3, on the read path.
Fix: the ListTagsOfResource handler's error construction (shared ARN-addressed not-found renderer with causes 3 and 5).
Traces: dynamodb-table-tagging-lifecycle#task#9.

Cause 5 of 5: UntagResource's not-found error has the same wrong identifier.
AWS: as cause 3, from UntagResource's required ResourceArn member.
Emulator: 'Requested resource not found: Table: <table-name> not found'.
Why: as cause 3, on the untag path.
Fix: the UntagResource handler's error construction (shared renderer).
Traces: dynamodb-table-tagging-lifecycle#task#11.

Not filed with this bundle (and deliberately so): divergence 38, ListTagsOfResource at dynamodb-tagging-and-resource-policy#task#5 returning three tags where the recording shows none - ListTagsOfResource is eventually consistent, so the recorded empty page is a timing artifact rather than a requirement.

## Evidence
Divergence 30 (TagResource, status 400->200): status expected 400 got 200; body expected {'__type': 'com.amazon.coral.validate#ValidationException', 'message': 'Duplicate Tag Keys provided as input: Duplicate Tag Key found env'}, got {} (empty).
Divergence 42 (ListTagsOfResource, 400->400): $.__type expected 'com.amazon.coral.validate#ValidationException', got 'ResourceNotFoundException'; $.message expected 'Invalid TableArn: Invalid ResourceArn provided as input arn:aws:dynamodb:us-east-1:000000000000:table/bad', got 'Requested resource not found: Table: bad not found'.
Divergences 35/36/37 (TagResource / ListTagsOfResource / UntagResource, 400->400): $.message expected 'Requested resource not found: ResourceArn: arn:aws:dynamodb:us-west-2:000000000000:table/<name>-missing not found', got 'Requested resource not found: Table: <name>-missing not found' - the submitted ResourceArn is replaced by a derived table name.
Model: smithy_query(operation=TagResource,ListTagsOfResource) -> TagResource input {ResourceArn: ResourceArnString required @length 1..1283, Tags: TagList required}, output Unit, errors incl. ResourceNotFoundException (400); ListTagsOfResource input {ResourceArn required, NextToken}, errors incl. ResourceNotFoundException (400).

## Steps to Reproduce
1. Replay dynamodb-resource-tag-lifecycle#task#7: TagResource with Tags containing the key 'env' twice -> 200 instead of 400 ValidationException.
2. Replay dynamodb-tagging-and-resource-policy#task#16: ListTagsOfResource with ResourceArn 'arn:aws:dynamodb:us-east-1:000000000000:table/bad' -> __type ResourceNotFoundException instead of com.amazon.coral.validate#ValidationException.
3. Replay dynamodb-table-tagging-lifecycle#task#8, #task#9 and #task#11 (TagResource / ListTagsOfResource / UntagResource against a non-existent table ARN): each 400 message names 'Table: <name>' instead of 'ResourceArn: <the submitted ARN>'.
