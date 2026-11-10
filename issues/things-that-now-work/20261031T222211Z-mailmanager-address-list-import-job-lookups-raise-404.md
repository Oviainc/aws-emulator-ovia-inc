Issue-Source: trace-sweep-divergence
Issue-Trace: mailmanager-address-list-import-job-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mailmanager-address-list-import-job-lifecycle
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|CreateAddressListImportJob|4xx->4xx|$.Message
Issue-Key: mailmanager|CreateAddressListImportJob|4xx->4xx|status
Issue-Key: mailmanager|GetAddressListImportJob|4xx->4xx|$.Message
Issue-Key: mailmanager|GetAddressListImportJob|4xx->4xx|$.__type
Issue-Key: mailmanager|GetAddressListImportJob|4xx->4xx|status
Issue-Commit: 7196227947f27242e011086dbc1a020fa1fe08f1

# mailmanager address-list import job lookups raise 404 ResourceNotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `7196227947f27242e011086dbc1a020fa1fe08f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddressListImportJob

## Description
The mailmanager address-list import-job operations classify an unknown resource id as a not-found condition (HTTP 404 / ResourceNotFoundException) where AWS classifies it as a request-validation failure (HTTP 400 / ValidationException) and words the message differently. Both operations are modeled with BOTH errors (ValidationException httpError 400 and ResourceNotFoundException httpError 404), so the emulator picks a modeled-but-wrong error class; the recorded AWS responses show which class AWS actually uses for these two lookups. 2 causes.

Cause 1 of 2: CreateAddressListImportJob with an AddressListId that does not exist.
  AWS: recorded behavior only (the Smithy model binds CreateAddressListImportJob to AccessDeniedException/403, ResourceNotFoundException/404, ThrottlingException/429, ValidationException/400 and does not say which applies to an unknown AddressListId). The recording shows HTTP 400 with the ValidationException body and Message "Could not find AddressList with id <AddressListId>". Fixed text: "Could not find AddressList with id "; variable part: the AddressListId copied verbatim from the request (the recording's al-sim2realprobemmmissingal586ac3d5 is just that run's value, not part of the template).
  Emulator: HTTP 404 with Message "Address list <id> not found." — wrong status, wrong error class, and wrong message text (different fixed wording plus a trailing period).
  Why: an SDK/client that catches ValidationException (or branches on a 4xx 400 validation failure) to report a bad AddressListId input sees an unhandled ResourceNotFoundException/404 instead; retry and error-mapping logic keyed on the modeled error class takes the wrong branch, and any message matching on "Could not find AddressList with id" fails.
  Fix: the CreateAddressListImportJob handler's address-list resolution path in the mailmanager service crate — the branch that maps a missing address list to an error must raise ValidationException (400) with the AWS wording, not ResourceNotFoundException (404).
  Traces: mailmanager-address-list-import-job-lifecycle#task#5.

Cause 2 of 2: GetAddressListImportJob with a JobId that does not exist.
  AWS: recorded behavior only (same four modeled errors; JobId is @required with length 1..255 and pattern ^[a-zA-Z0-9-]+$, and the probed id satisfies that pattern, so this is not a syntactic rejection). The recording shows HTTP 400, body __type "com.amazon.pancetta#ValidationException", Message "Invalid JobId" — the message is entirely fixed text with no variable part: AWS does not echo the job id.
  Emulator: HTTP 404, body __type "ResourceNotFoundException", Message "Import job <jobId> not found." — wrong status, wrong __type, and a message that both uses different fixed text and leaks the requested job id that AWS deliberately does not echo.
  Why: a client distinguishing "this job id is not acceptable" (ValidationException) from "this job is gone" (ResourceNotFoundException) mis-classifies the response; polling loops that treat 404 as terminal-deleted versus 400 as caller error diverge, and the __type string the SDK uses to construct the typed exception is wrong, so the modeled ValidationException is never thrown.
  Fix: the GetAddressListImportJob handler's job-lookup error path in the mailmanager service crate (error construction/serialization for the unknown-JobId case), which must emit ValidationException with __type com.amazon.pancetta#ValidationException, status 400 and the fixed message "Invalid JobId".
  Traces: mailmanager-address-list-import-job-lifecycle#task#6.

Related but distinct open issues (neither covers these causes): 20261029T234859Z-mailmanager-archive-search-and-export-reads-refused-with.md (archive search/export reads refused with 400 — it already owns the ListArchiveExports/StartArchiveSearch/ListArchiveSearches divergences in this corpus) and 20261030T011318Z-mailmanager-createarchive-mints-an-archiveid-that-violates-the-modeled.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200).

Cause 1 — fixture mailmanager-address-list-import-job-lifecycle, step task#5, operation CreateAddressListImportJob, verdict mismatch:
  status: expected 400, got 404
  $.Message: expected 'Could not find AddressList with id al-sim2realprobemmmissingal586ac3d5', got 'Address list al-sim2realprobemmmissingal586ac3d5 not found.'

Cause 2 — fixture mailmanager-address-list-import-job-lifecycle, step task#6, operation GetAddressListImportJob, verdict mismatch:
  status: expected 400, got 404
  $.__type: expected 'com.amazon.pancetta#ValidationException', got 'ResourceNotFoundException'
  $.Message: expected 'Invalid JobId', got 'Import job sim2real-probe-mm-missing-job-c9b591952f82 not found.'

Model evidence (services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json): CreateAddressListImportJob and GetAddressListImportJob each bind errors ValidationException (client, httpError 400, member Message) and ResourceNotFoundException (client, httpError 404, member Message); GetAddressListImportJob.JobId is required, @length 1..255, @pattern ^[a-zA-Z0-9-]+$.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture mailmanager-address-list-import-job-lifecycle.
3. Cause 1 (step task#5): call CreateAddressListImportJob with an AddressListId that was never created (e.g. a well-formed id matching ^[a-zA-Z0-9-]+$ such as al-<random>), a valid Name and an ImportDataFormat. Expected: HTTP 400, ValidationException, Message 'Could not find AddressList with id <that AddressListId>'. Observed: HTTP 404 with Message 'Address list <that AddressListId> not found.'
4. Cause 2 (step task#6): call GetAddressListImportJob with a JobId that was never created but matches ^[a-zA-Z0-9-]+$. Expected: HTTP 400, __type 'com.amazon.pancetta#ValidationException', Message 'Invalid JobId'. Observed: HTTP 404, __type 'ResourceNotFoundException', Message 'Import job <that JobId> not found.'
