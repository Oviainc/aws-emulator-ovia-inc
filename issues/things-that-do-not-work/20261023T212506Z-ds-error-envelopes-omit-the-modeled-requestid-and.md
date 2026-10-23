Issue-Source: trace-sweep-divergence
Issue-Trace: ds-directory-delete-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ds-directory-delete-error-paths ds-not-found-error-paths ds-account-directory-inventory-reads ds-conditional-forwarder-and-region-not-found-errors ds-directory-error-taxonomy ds-snapshot-error-paths ds-tags-error-paths-fake-directory
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|DeleteDirectory|4xx->4xx|$.Message
Issue-Key: ds|DeleteDirectory|4xx->4xx|$.RequestId
Issue-Key: ds|GetDirectoryLimits|2xx->2xx|$.DirectoryLimits.CloudOnlyMicrosoftADLimit
Issue-Key: ds|GetDirectoryLimits|2xx->2xx|$.DirectoryLimits.IdentityPoolDirectoriesLimitReached
Issue-Key: ds|RemoveRegion|4xx->4xx|$.Message
Issue-Key: ds|RemoveRegion|4xx->4xx|$.RequestId
Issue-Key: ds|DescribeDirectories|4xx->4xx|$.Message
Issue-Key: ds|DescribeDirectories|4xx->4xx|$.RequestId
Issue-Key: ds|GetSnapshotLimits|4xx->4xx|$.Message
Issue-Key: ds|GetSnapshotLimits|4xx->4xx|$.RequestId
Issue-Key: ds|DescribeClientAuthenticationSettings|4xx->4xx|$.Message
Issue-Key: ds|DescribeClientAuthenticationSettings|4xx->4xx|$.RequestId
Issue-Key: ds|CreateAlias|4xx->4xx|$.__type
Issue-Key: ds|CreateAlias|4xx->4xx|$.message
Issue-Key: ds|CreateSnapshot|4xx->4xx|$.Message
Issue-Key: ds|CreateSnapshot|4xx->4xx|$.RequestId
Issue-Key: ds|AddTagsToResource|4xx->4xx|$.Message
Issue-Key: ds|AddTagsToResource|4xx->4xx|$.RequestId
Issue-Key: ds|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: ds|ListTagsForResource|4xx->4xx|$.RequestId
Issue-Key: ds|RemoveTagsFromResource|4xx->4xx|$.Message
Issue-Key: ds|RemoveTagsFromResource|4xx->4xx|$.RequestId
Issue-Commit: 84e4c8a49718449b744f7486d6db0c693effe22b

# ds error envelopes omit the modeled RequestId and diverge on not-found text, CreateAlias classification and directory limits (5 causes)

> _Note: this issue was identified against the code base at commit `84e4c8a49718449b744f7486d6db0c693effe22b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDirectory, AddTagsToResource, ListTagsForResource, RemoveTagsFromResource, RemoveRegion, CreateSnapshot, GetSnapshotLimits, DescribeDirectories, DescribeClientAuthenticationSettings, CreateAlias, GetDirectoryLimits

## Description
Wire replay of the ds corpus (simparity-wire-aws-v1) produced 11 divergence clusters across 44 steps, all on the Directory Service (ds) surface and all on requests the emulator does answer (no stub involved). They reduce to FIVE independent root causes. A fixer must close all five before submitting.

Note: the open behavioral issue "ds: directory creation skips password and VPC validation, CreateAlias validates after resolution, error envelopes carry no RequestId and no ds Service Quotas are published" (20261022T175927Z-ds-directory-creation-skips-password-and-vpc-validation.md) describes overlapping ground from the BEHAVIORAL corpus, but it carries none of the wire replay keys below, so these wire steps are not gated by it and are filed here.

Cause 1 of 5: every ds error response omits the modeled RequestId member, and the error Message omits AWS's " : RequestId: <id>" suffix.
AWS: Smithy: every ds error structure models a RequestId member next to Message - EntityDoesNotExistException, ClientException, DirectoryDoesNotExistException and InvalidParameterException each declare RequestId with @pattern ^([A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12})$. The recorded responses carry that member in the JSON error body on every refusal path and additionally append the same id to the message as " : RequestId: <id>" (recorded example, DeleteDirectory on an absent id: Message "Directory: 'd-0000000000' does not exist. : RequestId: d52e2d42-59fe-495e-84fb-647afaf621a8", RequestId "d52e2d42-59fe-495e-84fb-647afaf621a8"). The requirement is the PRESENCE of a UUID-shaped RequestId member and the suffix rule; the particular id in any recording is an example and differs per request.
Emulator: HTTP status and __type match, but $.RequestId is absent (got None) on every one of these refusals, and no message carries the " : RequestId: " suffix. Observed on DeleteDirectory (400), CreateSnapshot (400), GetSnapshotLimits (400), DescribeDirectories (400 invalid NextToken), DescribeClientAuthenticationSettings (400), RemoveRegion (400), AddTagsToResource / ListTagsForResource / RemoveTagsFromResource (400).
Why: the request id is the only correlation handle an AWS client, SDK retry log or support workflow has for a failed ds call; botocore surfaces it from this member, so every caller reading e.response['Error']['RequestId'] or logging the message suffix gets nothing, and failures become untraceable.
Fix: the ds error serializer / error-envelope construction shared by all ds operation handlers - populate the modeled RequestId member from the per-request id and append the " : RequestId: <id>" suffix when building Message.
Traces: ds-directory-delete-error-paths#task#1 (also ds-not-found-error-paths#task#6), ds-snapshot-error-paths#task#1, ds-directory-error-taxonomy#task#1, ds-directory-error-taxonomy#task#2, ds-directory-error-taxonomy#task#3, ds-conditional-forwarder-and-region-not-found-errors#task#7, ds-tags-error-paths-fake-directory#task#1, ds-tags-error-paths-fake-directory#task#2, ds-tags-error-paths-fake-directory#task#3.

Cause 2 of 5: the tag API answers an unknown resource with the directory-not-found diagnostic instead of the tag-family "Invalid resource ID." diagnostic.
AWS: Smithy: AddTagsToResource / ListTagsForResource / RemoveTagsFromResource take ResourceId with @pattern ^[d]-[0-9a-f]{10}$ and model EntityDoesNotExistException (client, HTTP 400). Recorded behavior: for a well-formed but absent resource id these three operations answer 400 EntityDoesNotExistException with the tag-family message "Invalid resource ID." (plus the RequestId suffix of Cause 1) - the message does NOT name or quote the directory id. That rule is distinct from the directory-lookup family, which does quote the id.
Emulator: all three answer 400 with Message "Directory: 'd-0000000000' does not exist." - the directory-lookup template - so the tag-family diagnostic is never produced.
Why: callers and log scrapers key on these two distinct ds error families to tell "the id you passed is not a taggable resource id" from "this directory is gone"; collapsing them into one template makes a tagging bug read as a deleted directory and sends clients down a recreate path instead of fixing the id they passed.
Fix: the not-found message construction in the AddTagsToResource / ListTagsForResource / RemoveTagsFromResource handlers - use the tag-family diagnostic rather than falling through to the shared directory-lookup message.
Traces: ds-tags-error-paths-fake-directory#task#1 (AddTagsToResource), ds-tags-error-paths-fake-directory#task#2 (ListTagsForResource), ds-tags-error-paths-fake-directory#task#3 (RemoveTagsFromResource).

Cause 3 of 5: RemoveRegion's not-found message is region-qualified, diverging from the request-derived template.
AWS: Smithy: RemoveRegion input is DirectoryId only (@pattern ^d-[0-9a-f]{10}$) and the operation models DirectoryDoesNotExistException (client, HTTP 400). Recorded behavior: an absent directory is refused 400 with the message built from the request's directory id alone - "Directory: '<DirectoryId>' does not exist." plus the RequestId suffix (recorded example: "Directory: 'd-1234567890' does not exist. : RequestId: 3200ae60-e03a-4438-870d-fc318d395dc9"). The rule is request-derived: it names the id that was passed and nothing else.
Emulator: answers 400 with "Directory: 'd-1234567890' does not exist in us-west-2." - appending a region qualifier that is not part of the request-derived rule.
Why: the message is built from the request, so a client (or a multi-region test asserting the refusal) sees text that varies with the emulator's configured region rather than with its own input, and the diagnostic stops matching the family's template.
Fix: the RemoveRegion handler's not-found message construction - build the diagnostic from the request's DirectoryId without the region qualifier.
Traces: ds-conditional-forwarder-and-region-not-found-errors#task#7.

Cause 4 of 5: CreateAlias resolves the directory before validating the Alias member, so a malformed alias is misclassified.
AWS: Smithy: CreateAliasRequest.Alias targets AliasName with @length 1-62 and @pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$. A pattern violation is a framework validation failure raised BEFORE the directory lookup: HTTP 400 with __type "ValidationException" and the lowercase "message" member carrying the request-derived validation text that names the member and the violated constraint (recorded example: "1 validation error detected: Value 'SIM2REAL_Invalid_Alias' at 'alias' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*"). ValidationException is not in CreateAlias's modeled error list precisely because it is produced ahead of the operation's own resolution errors.
Emulator: answers 400 with __type "EntityDoesNotExistException" and an uppercase "Message" member; $.message is absent - i.e. the directory lookup ran first and the alias pattern was never checked.
Why: the __type is the field every SDK maps to an exception class, so a caller catches EntityDoesNotExistException and retries or recreates a directory in response to what is really a malformed-alias bug; it also changes the error-member casing (message vs Message), so generic validation-error handling finds no message at all.
Fix: the CreateAlias handler - run Alias member validation (length + pattern) ahead of directory resolution, and emit the framework validation envelope (__type ValidationException, lowercase message naming the member and the constraint).
Traces: ds-directory-error-taxonomy#task#4.

Cause 5 of 5: GetDirectoryLimits reports the wrong Managed Microsoft AD limit and omits a member the recorded response carries.
AWS: Smithy: GetDirectoryLimits returns DirectoryLimits, whose CloudOnlyMicrosoftADLimit is "The maximum number of Managed Microsoft AD directories allowed in the region". The model fixes no value, so recorded behavior only: a fresh account/region reports CloudOnlyMicrosoftADLimit 20. The recording also carries DirectoryLimits.IdentityPoolDirectoriesLimitReached = false, a member the checked-in model does not declare (recorded behavior only).
Emulator: returns 200 with CloudOnlyMicrosoftADLimit 10 and no IdentityPoolDirectoriesLimitReached key at all.
Why: this operation exists so a client can size its directory fleet against the account's quota before creating; a limit reported at half the real ceiling makes a caller refuse creations AWS would allow, and a missing limit-reached flag makes any code reading it see None instead of a boolean.
Fix: the GetDirectoryLimits handler / the directory-limits seed data in ds state - correct the Managed Microsoft AD ceiling and emit the full recorded DirectoryLimits member set including the identity-pool limit-reached flag.
Traces: ds-account-directory-inventory-reads#task#2.

## Evidence
Replay summary (trace_set_id simparity-wire-aws-v1): emulator_up true, bootstrap_signal false, state_reset_failures 0, 11 fixtures replayed, 44 steps - 3 passed, 41 mismatched, 11 divergence clusters, score 6.8%. All 11 clusters carry service "ds" and verdict "mismatch" with empty stub_path and empty owned_by.

Cause 1 (RequestId omitted on every error path), expected vs actual per cluster - status matched (400) in all cases:
- ds-directory-delete-error-paths#task#1 (DeleteDirectory; affects ds-not-found-error-paths#task#6): $.RequestId missing (expected 'd52e2d42-59fe-495e-84fb-647afaf621a8', got None); $.Message "Directory: 'd-0000000000' does not exist. : RequestId: d52e2d42-..." vs "Directory: 'd-0000000000' does not exist." (suffix dropped).
- ds-snapshot-error-paths#task#1 (CreateSnapshot): $.RequestId missing (expected '9dd527e2-7c83-4783-b493-c769a0c04807', got None); same dropped suffix.
- ds-directory-error-taxonomy#task#1 (DescribeDirectories, invalid NextToken), #task#2 (GetSnapshotLimits), #task#3 (DescribeClientAuthenticationSettings): $.RequestId missing (expected '<shaped-not-asserted>', got None) - the fixture asserts only that a request id of the modeled shape is present; the emulator supplies none.
- ds-conditional-forwarder-and-region-not-found-errors#task#7 (RemoveRegion): $.RequestId missing (expected '3200ae60-e03a-4438-870d-fc318d395dc9', got None).
- ds-tags-error-paths-fake-directory#task#1/#task#2/#task#3: $.RequestId missing (expected 'f4a44547-8d53-4939-9c8a-7fc6f361d17a' / 'c17e966e-e66b-4782-ae0f-c10c2cc2d540' / '1e769519-ea4e-480c-adeb-af3c12544077', got None).

Cause 2 (tag-family not-found text), 400 -> 400:
- ds-tags-error-paths-fake-directory#task#1 AddTagsToResource, #task#2 ListTagsForResource, #task#3 RemoveTagsFromResource: $.Message expected 'Invalid resource ID. : RequestId: <id>', got "Directory: 'd-0000000000' does not exist."

Cause 3 (RemoveRegion region-qualified text), 400 -> 400:
- ds-conditional-forwarder-and-region-not-found-errors#task#7: $.Message expected "Directory: 'd-1234567890' does not exist. : RequestId: 3200ae60-...", got "Directory: 'd-1234567890' does not exist in us-west-2."

Cause 4 (CreateAlias misclassification), 400 -> 400:
- ds-directory-error-taxonomy#task#4: $.__type expected 'ValidationException', got 'EntityDoesNotExistException'; $.message missing (expected "1 validation error detected: Value 'SIM2REAL_Invalid_Alias' at 'alias' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*", got None).

Cause 5 (GetDirectoryLimits), 200 -> 200:
- ds-account-directory-inventory-reads#task#2: $.DirectoryLimits.CloudOnlyMicrosoftADLimit value_mismatch (expected 20, got 10); $.DirectoryLimits.IdentityPoolDirectoriesLimitReached missing (expected False, got None).

Model references used: Smithy GetDirectoryLimits -> DirectoryLimits (CloudOnlyMicrosoftADLimit doc "maximum number of Managed Microsoft AD directories allowed in the region"; no IdentityPoolDirectoriesLimitReached member declared); CreateAlias input Alias @length 1-62 @pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$, errors ClientException/EntityAlreadyExistsException/EntityDoesNotExistException/InvalidParameterException/ServiceException; AddTagsToResource/ListTagsForResource/RemoveTagsFromResource ResourceId @pattern ^[d]-[0-9a-f]{10}$ with EntityDoesNotExistException; RemoveRegion input DirectoryId only, errors include DirectoryDoesNotExistException; error shapes EntityDoesNotExistException / ClientException / DirectoryDoesNotExistException / InvalidParameterException all declare members {Message, RequestId @pattern UUID}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; replay the ds wire corpus with replay_wire_corpus() (trace set simparity-wire-aws-v1).
2. Cause 1 - RequestId: replay fixture ds-directory-delete-error-paths step task#1 (DeleteDirectory with DirectoryId 'd-0000000000'). Inspect the 400 JSON body: $.RequestId is absent and $.Message has no ' : RequestId: <id>' suffix. Repeat for ds-snapshot-error-paths#task#1, ds-directory-error-taxonomy#task#1/#task#2/#task#3, ds-conditional-forwarder-and-region-not-found-errors#task#7 and ds-tags-error-paths-fake-directory#task#1/#task#2/#task#3 - every ds refusal in the corpus is missing the member.
3. Cause 2 - tag family: replay ds-tags-error-paths-fake-directory steps task#1, task#2, task#3 (AddTagsToResource / ListTagsForResource / RemoveTagsFromResource against ResourceId 'd-0000000000'). Expect 400 EntityDoesNotExistException with the tag-family message 'Invalid resource ID.'; observe the directory-lookup template "Directory: 'd-0000000000' does not exist."
4. Cause 3 - RemoveRegion: replay ds-conditional-forwarder-and-region-not-found-errors step task#7 (RemoveRegion DirectoryId 'd-1234567890'). Expect the message built from the request id alone; observe the ' in <region>' qualifier appended.
5. Cause 4 - CreateAlias ordering: replay ds-directory-error-taxonomy step task#4 (CreateAlias with an Alias containing underscores against a never-created DirectoryId). Expect 400 __type ValidationException with a lowercase 'message' naming 'alias' and the modeled pattern; observe __type EntityDoesNotExistException with 'Message' and no 'message'.
6. Cause 5 - GetDirectoryLimits: replay ds-account-directory-inventory-reads step task#2. Expect 200 with DirectoryLimits.CloudOnlyMicrosoftADLimit 20 and DirectoryLimits.IdentityPoolDirectoriesLimitReached false; observe 10 and the key absent.
7. After fixing, re-replay the representative step (ds-directory-delete-error-paths#task#1) AND every fixture#step listed in this issue's trace ids - each cause has its own cluster.
