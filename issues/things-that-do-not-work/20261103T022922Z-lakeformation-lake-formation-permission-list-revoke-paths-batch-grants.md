Issue-Source: trace-sweep-divergence
Issue-Trace: lakeformation-gtr-lf-batch-grant-partial-failure
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lakeformation-gtr-lf-batch-grant-partial-failure lakeformation-gtr-lf-role-grant-fix-on-governed-table
Issue-Shape-Id: com.amazonaws.lakeformation#AWSLakeFormation
Issue-Key: lakeformation|ListPermissions|2xx->2xx|$.PrincipalResourcePermissions[*]
Issue-Key: lakeformation|ListPermissions|4xx->2xx|$.Message
Issue-Key: lakeformation|ListPermissions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lakeformation|ListPermissions|4xx->2xx|status
Issue-Key: lakeformation|ListPermissions|4xx->4xx|$.Message
Issue-Key: lakeformation|RevokePermissions|4xx->4xx|$.Message
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# Lake Formation permission list/revoke paths: batch grants unrecorded, principal ARN unvalidated, refusal texts emulator-authored

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPermissions, RevokePermissions

## Description
Four net-new causes on the Lake Formation permission list/revoke surface: a permission granted through BatchGrantPermissions is never readable back, ListPermissions accepts a principal identifier AWS refuses, and two 400 refusals on this surface carry sentences the emulator composed instead of the fixed text AWS returns. All four live in the same permission handler/validation path of the lakeformation crate. 4 causes.

Cause 1 of 4 — ListPermissions does not return a permission that BatchGrantPermissions accepted.
AWS: Smithy: BatchGrantPermissions(Entries: BatchPermissionsRequestEntryList, required) -> output Failures: BatchPermissionsFailureList; a partial-failure batch APPLIES every entry that did not fail and reports only the failed entries in Failures. ListPermissions -> PrincipalResourcePermissions: PrincipalResourcePermissionsList, filtered by the request's Principal/Resource. Recorded response: HTTP 200 with one PrincipalResourcePermissions entry whose Permissions is ['DESCRIBE'], PermissionsWithGrantOption is [], Principal.DataLakePrincipalIdentifier is the grantee role ARN, and Resource.Table names the CatalogId, DatabaseName and table Name that the successful batch entry granted on.
Emulator: HTTP 200 (status correct) with an EMPTY PrincipalResourcePermissions list — $.PrincipalResourcePermissions[0] is missing entirely, so the entry the batch accepted is absent.
Why: a client that grants in bulk and then audits or diffs access is told the grantee holds nothing, so revoke/diff flows have nothing to act on and an entitlement check that passed on AWS fails here; the permission is effectively lost even though the grant call reported it as not-failed.
Fix: the BatchGrantPermissions handler and the permission store it writes into (lakeformation crate state), plus the ListPermissions read path that filters by Principal/Resource — a non-failed batch entry must be persisted and returned. Related but NOT covering this: open issue 20261102T091623Z-lakeformation-listpermissions-returns-nothing-for-the-creator-of.md concerns IMPLICIT creator grants on a governed table, not an explicitly batch-granted permission.
Traces: lakeformation-gtr-lf-batch-grant-partial-failure#task#5.

Cause 2 of 4 — ListPermissions accepts a non-ARN Principal instead of refusing it.
AWS: Smithy: ListPermissions.Principal targets DataLakePrincipal (member DataLakePrincipalIdentifier); the operation models InvalidInputException (client, httpError 400). Recorded response: HTTP 400 with {"Message": "Invalid ARN:ARNs must start with 'arn:': <principalIdentifier>"} and an x-amzn-errortype header — the fixed text is "Invalid ARN:ARNs must start with 'arn:': " and the only variable part is the rejected DataLakePrincipalIdentifier taken verbatim from the request (in the recording, a bare role name with no arn: prefix).
Emulator: HTTP 200 with a success body — no status 400, no $.Message, no x-amzn-errortype header. The malformed principal is accepted and the call reports success.
Why: a client that passes a bare role name (a very common mistake) gets a successful empty listing instead of the InvalidInputException AWS raises, so it concludes the principal legitimately holds no permissions rather than that its request was malformed — a silent false negative in any access audit.
Fix: the ListPermissions handler's Principal validation in the lakeformation crate — validate DataLakePrincipalIdentifier has ARN shape before querying, and raise InvalidInputException with AWS's ARN-shape template.
Traces: lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#5.

Cause 3 of 4 — ListPermissions' both-Name-and-TableWildcard refusal is reworded.
AWS: Smithy: ListPermissions.Resource targets Resource, whose Table member is a TableResource carrying both Name and TableWildcard; the operation models InvalidInputException (client, httpError 400). Recorded response: HTTP 400 with {"Message": "Table name and table wildcard cannot both be present."} — the whole sentence is FIXED text including the trailing period, with no variable part (neither the table nor the database name is echoed).
Emulator: HTTP 400 (status correct) with $.Message = "Resource.Table must name either Name or TableWildcard, not both" — an emulator-authored sentence naming the member path instead of AWS's wording.
Why: the status and class are right but the text is not AWS's, so a caller that surfaces or matches on the refusal sentence (log scraping, branch logic, pinned test assertions) cannot recognise the mutually-exclusive-field case.
Fix: the ListPermissions resource validation in the lakeformation crate — emit AWS's fixed sentence as a constant.
Traces: lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#6.

Cause 4 of 4 — RevokePermissions' nothing-to-revoke refusal is reworded and echoes the requested permission list.
AWS: Smithy: RevokePermissions(Principal, Resource, Permissions required) models InvalidInputException (client, httpError 400) and EntityNotFoundException. Recorded response: HTTP 400 with {"Message": "No permissions revoked. Grantee has no permissions and no grantable permissions on resource."} — the whole message is FIXED text (two sentences, trailing period); the requested Permissions values are NOT interpolated, because the request already carries them.
Emulator: HTTP 400 (status correct) with $.Message = "No permissions revoked. Grantee does not does not have:[<requestedPermissions>]" — a different sentence that splices the request's Permissions list in, and that contains the duplicated words "does not does not".
Why: a revoke-if-present clean-up path that tolerates this specific refusal by matching AWS's sentence instead sees an unknown failure and aborts; the garbled duplication is also surfaced to end users.
Fix: the RevokePermissions handler's no-op refusal path in the lakeformation crate — emit AWS's fixed sentence, drop the permission-list interpolation.
Traces: lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200, score 60.1%). All four clusters are verdict=mismatch and none is owned by an open issue or blocked by a stub.

1. lakeformation-gtr-lf-batch-grant-partial-failure#task#5 ListPermissions 200->200: $.PrincipalResourcePermissions[0] missing — expected one entry {Permissions: ['DESCRIBE'], PermissionsWithGrantOption: [], Principal.DataLakePrincipalIdentifier: <grantee role ARN>, Resource.Table: {CatalogId, DatabaseName, Name}}, got None (empty list).
2. lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#5 ListPermissions expected 400, got 200: $.Message missing (expected "Invalid ARN:ARNs must start with 'arn:': <principalIdentifier>", got None); $headers.x-amzn-errortype missing.
3. lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#6 ListPermissions 400->400: $.Message expected 'Table name and table wildcard cannot both be present.', got 'Resource.Table must name either Name or TableWildcard, not both'.
4. lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#11 RevokePermissions 400->400: $.Message expected 'No permissions revoked. Grantee has no permissions and no grantable permissions on resource.', got 'No permissions revoked. Grantee does not does not have:[<requestedPermissions>]'.

The open issues 20261029T173933Z-lakeformation-lake-formation-client-error-bodies-reword-or-drop.md and 20261029T173933Z-lakeformation-lake-formation-reads-omit-modeled-members-and-accept.md carry replay keys for other Lake Formation operations (data cells filters, LF tags, resource registration, data lake settings, GrantPermissions) and do NOT cover these four ListPermissions/RevokePermissions/BatchGrantPermissions clusters.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or drive the fixtures directly:
   - lakeformation-gtr-lf-batch-grant-partial-failure: run the fixture up to its BatchGrantPermissions call (one entry valid, one failing), then ListPermissions filtered by the grantee role ARN and the granted table resource (step task#5); expect one DESCRIBE entry, observe an empty PrincipalResourcePermissions list.
   - lakeformation-gtr-lf-role-grant-fix-on-governed-table: call ListPermissions with Principal.DataLakePrincipalIdentifier set to a bare role name (no 'arn:' prefix) (step task#5); expect 400 with AWS's ARN-shape message, observe 200.
   - same fixture: call ListPermissions with Resource.Table carrying BOTH Name and TableWildcard (step task#6) and compare $.Message against 'Table name and table wildcard cannot both be present.'.
   - same fixture: call RevokePermissions for a principal/resource pair holding no permissions (step task#11) and compare $.Message against 'No permissions revoked. Grantee has no permissions and no grantable permissions on resource.'.
3. Diff each response's status, x-amzn-errortype and $.Message (or $.PrincipalResourcePermissions) against the recorded AWS response.
