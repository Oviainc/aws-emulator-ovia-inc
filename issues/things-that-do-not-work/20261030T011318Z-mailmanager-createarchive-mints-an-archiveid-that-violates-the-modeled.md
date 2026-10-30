Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search mailmanager.multiple-sequential-archive-searches-are-each-tracked-independently
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|StartArchiveSearch|ValidationException
Issue-Commit: 6499d5c21f60bffb920d67d38c35c702f5411e4b

# CreateArchive mints an ArchiveId that violates the modeled ArchiveId pattern, so every archive-search operation refuses the id the emulator itself handed out

> _Note: this issue was identified against the code base at commit `6499d5c21f60bffb920d67d38c35c702f5411e4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateArchive

## Description
Cause 1 of 1: CreateArchive mints an identifier that no downstream archive operation can accept, making the whole archive-search lifecycle unreachable through the emulator's own API.

AWS: Smithy (services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json): CreateArchive output member ArchiveId targets ArchiveIdString (string, @length 1..66, no @pattern). The identifier that same value is later presented as is modeled by a DIFFERENT, tighter shape: ArchiveId (string, @length min 3 max 66, @pattern ^a-[\w]{1,64}$), which is the target of StartArchiveSearch.ArchiveId (required), ListArchiveSearches.ArchiveId (required), ListArchiveExports.ArchiveId (required) and GetArchiveSearch output ArchiveId. The two shapes describe the same identifier space, and the constrained one is authoritative about its lexical form: a real MailManager ArchiveId begins with the literal prefix 'a-' followed by 1..64 word characters. Consequently in AWS the id returned by CreateArchive always satisfies ArchiveId's pattern, and the documented flow CreateArchive -> StartArchiveSearch -> GetArchiveSearch -> GetArchiveSearchResults -> ListArchiveSearches -> DeleteArchive succeeds: StartArchiveSearch answers HTTP 200 with a SearchId (SearchId, @length 1..64). AWS reserves ValidationException (HTTP 400) for an ArchiveId the CALLER malformed, not for one the service minted.

Emulator: CreateArchive answers 200 with an ArchiveId of 32 lowercase hex characters and no 'a-' prefix — e.g. '2c809e531351e410910a2dec89025cc1' and 'f6bb9d5cc0cc5049910a2dec89025cc1' in the two replays (the hex digits are per-run values; the SHAPE, 32 hex characters with no prefix, is the defect). Feeding that id straight back into StartArchiveSearch is then refused with HTTP 400 ValidationException and the message '1 validation error detected: Value <archiveId> at \'archiveId\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^a-[\w]{1,64}$'. In that message the fixed text is '1 validation error detected: Value ', " at 'archiveId' failed to satisfy constraint: Member must satisfy regular expression pattern: " and the trailing pattern literal ^a-[\w]{1,64}$ (which comes from the model); the quoted value is the ArchiveId taken from the request. The boundary validation is CORRECT here — it is enforcing the modeled ArchiveId pattern faithfully. The defect is upstream: the id-minting in this service's state, crate::state::ComAmazonawsMailmanagerMailmanagersvcState::mint_archive_id, formats '{high:016x}{low:016x}' and its doc comment reasons only about ArchiveIdString's 1..66 length, never about the ArchiveId pattern the same value must later satisfy.

Why: a client that follows the documented flow — create an archive, then search it — cannot proceed past the second call. There is no value the client can supply instead: the only ArchiveId in existence is the one CreateArchive returned, and the service rejects it as malformed, blaming the caller for a value the caller never chose. Every archive-search and archive-export flow (StartArchiveSearch, GetArchiveSearch, GetArchiveSearchResults, ListArchiveSearches, ListArchiveExports) is therefore unusable, and because a 400 ValidationException is classified as a non-retryable client error, SDK retry and waiter logic aborts immediately rather than recovering. A client also cannot round-trip the id through its own storage or validation layers, since SDK-side constraint checks reject it for the same reason.

Fix: the ArchiveId minting in this service's state (mint_archive_id in services/service-com-amazonaws-mailmanager-mailmanagersvc/src/state.rs) must produce an identifier satisfying the modeled ArchiveId @pattern ^a-[\w]{1,64}$ and @length 3..66 — i.e. the literal 'a-' prefix followed by word characters — so the value CreateArchive returns is accepted by every operation that takes an ArchiveId. Keep the existing uniqueness property (the sequence-derived bijection) so two archives still cannot share an id. CreateArchive's handler needs no other change; the boundary validation that currently refuses the id must NOT be relaxed, since it is enforcing the model correctly. Note the open issue 20261029T234859Z-mailmanager-archive-search-and-export-reads-refused-with.md, filed from the WIRE corpus, reports the same 400 ValidationException symptom on StartArchiveSearch/ListArchiveSearches/ListArchiveExports but hypothesises the opposite cause ('boundary validation mis-evaluates that ArchiveId constraint ... and refuses an identifier AWS accepts'). This behavioral replay, which creates the archive in-band and feeds the emulator's own id back, shows the validation is right and the minted id is wrong; a fix that loosens the validation per that hypothesis would leave the emulator handing out ids that real AWS clients and SDK-side constraint checks still reject.

Traces: mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search, mailmanager.multiple-sequential-archive-searches-are-each-tracked-independently. Both fail on their first StartArchiveSearch call, immediately after a successful CreateArchive in the same check, so neither check ever reaches the behavior it was written to assert (post-deletion rejection wording, and independent tracking of several sequential searches). Those downstream assertions remain unverified until this is fixed.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator (emulator serving; 0/8 passed, 8 errored — 6 of them the ALREADY FILED CreateAddressList stub cluster, the remaining 2 are this cause).

1) check mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search, verdict error:
   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartArchiveSearch operation: 1 validation error detected: Value '2c809e531351e410910a2dec89025cc1' at 'archiveId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^a-[\w]{1,64}$
   The quoted value is the ArchiveId returned by the CreateArchive call two lines earlier in the same check:
     arc_id = mm.create_archive(ArchiveName=ctx.unique_name(...))['ArchiveId']
     mm.start_archive_search(ArchiveId=arc_id, FromTimestamp=..., ToTimestamp=..., MaxResults=10)

2) check mailmanager.multiple-sequential-archive-searches-are-each-tracked-independently, verdict error:
   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartArchiveSearch operation: 1 validation error detected: Value 'f6bb9d5cc0cc5049910a2dec89025cc1' at 'archiveId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^a-[\w]{1,64}$
   Same shape of id, same refusal, on the first of three sequential searches.

Expected (AWS, per the model): StartArchiveSearch answers HTTP 200 with a SearchId (SearchId @length 1..64) for the ArchiveId CreateArchive just returned; no ValidationException, because AWS's CreateArchive returns an id matching ArchiveId's @pattern ^a-[\w]{1,64}$.
Actual (emulator): HTTP 400 ValidationException on StartArchiveSearch for the emulator's own freshly minted ArchiveId.

Source of the malformed id (file/line citations only):
- services/service-com-amazonaws-mailmanager-mailmanagersvc/src/state.rs:145-151 — mint_archive_id formats the id as '{high:016x}{low:016x}' (32 hex chars, no 'a-' prefix); its doc comment at :139-144 justifies the form against ArchiveIdString's 1..66 length only.
- services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/create_archive.rs:230 — the handler calls store.mint_archive_id(now) and returns that value as CreateArchiveOutput.archive_id (:262).
- Model constraint being violated: ArchiveId = string, @length 3..66, @pattern ^a-[\w]{1,64}$; it is the target of StartArchiveSearch.ArchiveId, ListArchiveSearches.ArchiveId and GetArchiveSearch output ArchiveId.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Call CreateArchive with any valid ArchiveName (pattern ^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$, 1..64 chars). It answers 200 with an ArchiveId of 32 lowercase hex characters, with no 'a-' prefix.
3. Pass that exact ArchiveId straight back to StartArchiveSearch together with FromTimestamp, ToTimestamp and MaxResults=10. Observe HTTP 400 ValidationException: "1 validation error detected: Value '<the id from step 2>' at 'archiveId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^a-[\w]{1,64}$". AWS answers 200 with a SearchId.
4. The same refusal occurs for ListArchiveSearches and ListArchiveExports on that ArchiveId, since all three bind the constrained ArchiveId shape.
5. Replay the behavioral checks mailmanager.archive-deletion-rejects-further-reads-of-its-completed-search and mailmanager.multiple-sequential-archive-searches-are-each-tracked-independently. Both must run past StartArchiveSearch and then pass their own assertions (the first expects ValidationException 'Search not found.' from GetArchiveSearch and 'Archive is not active.' from ListArchiveSearches after the archive is deleted; the second expects three distinct SearchIds, each reaching COMPLETED, empty Rows from GetArchiveSearchResults, and ListArchiveSearches accounting for exactly those three ids) before this issue is closed.
