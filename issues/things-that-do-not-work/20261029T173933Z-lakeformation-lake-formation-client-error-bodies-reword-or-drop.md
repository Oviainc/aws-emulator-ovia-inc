Issue-Source: trace-sweep-divergence
Issue-Trace: lakeformation-data-cells-filter-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lakeformation-data-cells-filter-lifecycle lakeformation-data-lake-location-registration lakeformation-data-lake-settings-and-principal lakeformation-lf-tag-lifecycle lakeformation-table-permission-grant-revoke
Issue-Shape-Id: com.amazonaws.lakeformation#AWSLakeFormation
Issue-Key: lakeformation|GetDataCellsFilter|4xx->4xx|$.Message
Issue-Key: lakeformation|CreateDataCellsFilter|4xx->4xx|$.Message
Issue-Key: lakeformation|DescribeResource|4xx->4xx|$.Message
Issue-Key: lakeformation|RegisterResource|4xx->4xx|$.Message
Issue-Key: lakeformation|PutDataLakeSettings|4xx->4xx|$.Message
Issue-Key: lakeformation|GetLFTag|4xx->4xx|$.Message
Issue-Key: lakeformation|CreateLFTag|4xx->4xx|$.Message
Issue-Key: lakeformation|DeleteLFTag|4xx->4xx|$.Message
Issue-Key: lakeformation|GrantPermissions|4xx->4xx|$.Message
Issue-Key: lakeformation|GrantPermissions|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 42eb24959c6a4eb33ac3ba3d48b702ee83f6032d

# Lake Formation client-error bodies reword or drop AWS refusal text

> _Note: this issue was identified against the code base at commit `42eb24959c6a4eb33ac3ba3d48b702ee83f6032d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataCellsFilter, CreateDataCellsFilter, DescribeResource, RegisterResource, PutDataLakeSettings, GetLFTag, CreateLFTag, DeleteLFTag, GrantPermissions

## Description
The emulator's Lake Formation refusals carry the right HTTP status in 8 of 9 cases but the WRONG BODY: every message is prose the emulator composed itself (resource names, catalog ids and ARNs interpolated into a sentence AWS does not use), and in one case the modeled error shape is replaced and the Message key dropped entirely. Lake Formation's client errors are modeled with a single `Message` member and the service sends SHORT, FIXED sentences for these entity-state errors — the resource identity is already in the request, so AWS does not echo it. 9 distinct causes, all one root fault in how this service crate constructs error payloads.

Cause 1 of 9 — GetDataCellsFilter, filter absent.
AWS: Smithy: GetDataCellsFilter errors EntityNotFoundException (httpError 400, member Message). Recorded response: HTTP 400 with body {"Message": "Entity Not Found"} — the whole message is FIXED text, with no variable part; the filter/table/catalog identity is NOT interpolated.
Emulator: HTTP 400 (status correct) with $.Message = "Data cells filter <filterName> not found on table <databaseName>.<tableName> in catalog <catalogId>" — an emulator-authored sentence.
Why: a client matching the refusal text (log scrapers, retry/branch logic, test assertions pinned to the AWS string) sees an unknown message and cannot classify the not-found case the way it does against AWS.
Fix: the GetDataCellsFilter handler's EntityNotFoundException construction in the lakeformation service crate — emit the fixed text, drop the interpolation.
Traces: lakeformation-data-cells-filter-lifecycle#task#3.

Cause 2 of 9 — CreateDataCellsFilter on an existing filter.
AWS: Smithy: CreateDataCellsFilter errors AlreadyExistsException (httpError 400, member Message). Recorded: HTTP 400, {"Message": "Data cells filter already exists."} — fixed text including the trailing period, no variables.
Emulator: HTTP 400 with $.Message = "Data cells filter <name> already exists on table <database>.<table> in catalog <catalogId>".
Why: same as cause 1 — the already-exists branch is unrecognisable to a client keyed on AWS wording.
Fix: the CreateDataCellsFilter handler's AlreadyExistsException construction.
Traces: lakeformation-data-cells-filter-lifecycle#task#6 (cluster also covers task#7).

Cause 3 of 9 — DescribeResource on an unregistered location.
AWS: Smithy: DescribeResource errors EntityNotFoundException (httpError 400, member Message). Recorded: HTTP 400, {"Message": "Entity not found"} — fixed text, lower-case 'not found', no ARN echoed.
Emulator: HTTP 400 with $.Message = "Resource <resourceArn> is not registered".
Why: a client cannot distinguish 'never registered' from any other Lake Formation 400 by message, and the ARN echo is not part of AWS's contract.
Fix: the DescribeResource handler's not-found path.
Traces: lakeformation-data-lake-location-registration#task#4.

Cause 4 of 9 — RegisterResource on an already-registered location.
AWS: Smithy: RegisterResource errors AlreadyExistsException (httpError 400, member Message). Recorded: HTTP 400, {"Message": "Resource is already registered"} — fixed text, the ARN is NOT part of it.
Emulator: HTTP 400 with $.Message = "Resource <resourceArn> is already registered" (the ARN from the request spliced into the sentence).
Why: idempotent registration flows that match on AWS's sentence treat the emulator's reply as an unknown failure.
Fix: the RegisterResource handler's AlreadyExistsException construction.
Traces: lakeformation-data-lake-location-registration#task#7 (cluster also covers task#8).

Cause 5 of 9 — PutDataLakeSettings with a non-ARN admin principal.
AWS: Smithy: PutDataLakeSettings errors InvalidInputException (httpError 400, member Message); DataLakeSettings.DataLakeAdmins targets DataLakePrincipalList of DataLakePrincipal/DataLakePrincipalIdentifier. Recorded: HTTP 400, {"Message": "Invalid ARN:ARNs must start with 'arn:': <rejectedIdentifier>"} — the fixed part is "Invalid ARN:ARNs must start with 'arn:': " and the only variable part is the rejected DataLakePrincipalIdentifier taken from the request.
Emulator: HTTP 400 with $.Message = "DataLakeAdmins principal \"<identifier>\" is not supported: a data lake administrator is an IAM user or an IAM role" — a different sentence built from the model's doc prose.
Why: the refusal reason a client surfaces to its user (malformed ARN) is replaced by unrelated wording, and the quoting/format differs, so message-based validation reporting diverges.
Fix: the PutDataLakeSettings principal validation in the lakeformation service crate — use AWS's ARN-shape message template with the rejected identifier as its only substitution.
Traces: lakeformation-data-lake-settings-and-principal#task#7.

Cause 6 of 9 — GetLFTag for a tag key that does not exist.
AWS: Smithy: GetLFTag errors EntityNotFoundException (httpError 400, member Message). Recorded: HTTP 400, {"Message": "Tag key does not exist"} — fixed text, the key is not echoed.
Emulator: HTTP 400 with $.Message = "Tag key <tagKey> not found in catalog <catalogId>".
Why: as above — an LF-tag consumer branching on AWS's sentence mis-classifies the miss.
Fix: the GetLFTag handler's not-found path.
Traces: lakeformation-lf-tag-lifecycle#task#3.

Cause 7 of 9 — CreateLFTag for an existing tag key.
AWS: Smithy: CreateLFTag errors AlreadyExistsException (httpError 400, member Message). Recorded: HTTP 400, {"Message": "Tag key already exists"} — fixed text.
Emulator: HTTP 400 with $.Message = "Tag key <tagKey> already exists in catalog <catalogId>".
Why: same class of defect; create-if-absent logic keyed on the AWS string fails.
Fix: the CreateLFTag handler's AlreadyExistsException construction.
Traces: lakeformation-lf-tag-lifecycle#task#6.

Cause 8 of 9 — DeleteLFTag for an absent tag key.
AWS: Smithy: DeleteLFTag errors EntityNotFoundException (httpError 400, member Message). Recorded: HTTP 400, {"Message": "Tag key does not exist"} — the SAME fixed sentence GetLFTag uses.
Emulator: HTTP 400 with $.Message = "Tag key <tagKey> not found in catalog <catalogId>".
Why: clean-up paths that tolerate a missing tag by matching AWS's text now see an unknown failure.
Fix: the DeleteLFTag handler's not-found path (share the one message constant with GetLFTag).
Traces: lakeformation-lf-tag-lifecycle#task#11.

Cause 9 of 9 — GrantPermissions with an unmodeled permission value: wrong error shape AND no message at all.
AWS: Smithy: GrantPermissions errors are ConcurrentModificationException, EntityNotFoundException and InvalidInputException only — ValidationException is NOT bound to this operation, and Permission is an enum whose values are ALL, SELECT, ALTER, DROP, DELETE, INSERT, DESCRIBE, CREATE_DATABASE, CREATE_TABLE, DATA_LOCATION_ACCESS, CREATE_LF_TAG, ASSOCIATE, GRANT_WITH_LF_TAG_EXPRESSION, CREATE_LF_TAG_EXPRESSION, CREATE_CATALOG, SUPER_USER. Recorded: HTTP 400 with {"Message": " Please grant only valid Lake Formation permissions. [<rejectedPermissions>] are not all valid Lake Formation permissions. "} — the fixed text is the two sentences and the leading/trailing single spaces; the only variable part is the bracketed list of rejected permission values taken from the request's Permissions list.
Emulator: HTTP 400 but the body carries NO Message key at all, and x-amzn-errortype is "ValidationException" — an error type this operation does not model.
Why: the SDK deserialises an error the client's modeled union has no arm for, so typed `catch InvalidInputException` handling never fires and the reason is lost entirely (no message to log either).
Fix: the GrantPermissions permission-enum validation and its error serializer — raise InvalidInputException and populate Message with AWS's invalid-permissions template.
Traces: lakeformation-table-permission-grant-revoke#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200). All nine clusters are verdict=mismatch.

1. lakeformation-data-cells-filter-lifecycle#task#3 GetDataCellsFilter 400->400: $.Message expected 'Entity Not Found', got 'Data cells filter <name> not found on table <db>.<tbl> in catalog <catalogId>'.
2. lakeformation-data-cells-filter-lifecycle#task#6 (also task#7) CreateDataCellsFilter 400->400: $.Message expected 'Data cells filter already exists.', got 'Data cells filter <name> already exists on table <db>.<tbl> in catalog <catalogId>'.
3. lakeformation-data-lake-location-registration#task#4 DescribeResource 400->400: $.Message expected 'Entity not found', got 'Resource <arn> is not registered'.
4. lakeformation-data-lake-location-registration#task#7 (also task#8) RegisterResource 400->400: $.Message expected 'Resource is already registered', got 'Resource <arn> is already registered'.
5. lakeformation-data-lake-settings-and-principal#task#7 PutDataLakeSettings 400->400: $.Message expected "Invalid ARN:ARNs must start with 'arn:': <identifier>", got 'DataLakeAdmins principal "<identifier>" is not supported: a data lake administrator is an IAM user or an IAM role'.
6. lakeformation-lf-tag-lifecycle#task#3 GetLFTag 400->400: $.Message expected 'Tag key does not exist', got 'Tag key <key> not found in catalog <catalogId>'.
7. lakeformation-lf-tag-lifecycle#task#6 CreateLFTag 400->400: $.Message expected 'Tag key already exists', got 'Tag key <key> already exists in catalog <catalogId>'.
8. lakeformation-lf-tag-lifecycle#task#11 DeleteLFTag 400->400: $.Message expected 'Tag key does not exist', got 'Tag key <key> not found in catalog <catalogId>'.
9. lakeformation-table-permission-grant-revoke#task#8 GrantPermissions 400->400: $.Message missing (expected ' Please grant only valid Lake Formation permissions. [<rejected>] are not all valid Lake Formation permissions. ', got None); $headers.x-amzn-errortype shape_mismatch, got 'ValidationException' (not a modeled GrantPermissions error).

No open issue covers any of these; the existing open lakeformation-adjacent issue 20261025T000319Z-glue-iam-lakeformation-s3-a-glue-job-run.md is about a Glue job run and is unrelated.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or exercise the single fixtures directly:
   - lakeformation-data-cells-filter-lifecycle: GetDataCellsFilter for a filter that was never created (step task#3), then CreateDataCellsFilter twice with the same TableData (step task#6).
   - lakeformation-data-lake-location-registration: DescribeResource on an unregistered S3 ARN (step task#4), then RegisterResource twice for the same ARN (step task#7).
   - lakeformation-data-lake-settings-and-principal: PutDataLakeSettings with DataLakeAdmins=[{DataLakePrincipalIdentifier: 'not-an-arn-or-known-principal'}] (step task#7).
   - lakeformation-lf-tag-lifecycle: GetLFTag for an uncreated TagKey (task#3), CreateLFTag twice for the same TagKey (task#6), DeleteLFTag after the tag is deleted (task#11).
   - lakeformation-table-permission-grant-revoke: GrantPermissions with Permissions=['NOSUCHPERMISSION'] (task#10 numbering per fixture; the divergence is step task#8).
3. Compare each 400 body's $.Message (and x-amzn-errortype for GrantPermissions) against the recorded AWS response.
