Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# BatchGetFindings never classifies an identifier as INVALID_FINDING_ID and reports a well-formed-but-unknown scanName as INVALID_SCAN_NAME

_Filed: 2026-10-24T06:13:33Z_
_Operation: BatchGetFindings | Class: error-response_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: BatchGetFindings reports one `failedFindings` entry per identifier it cannot resolve, and the `errorCode` of that entry is the classification the model enumerates for the actual reason (`ErrorCode` = DUPLICATE_IDENTIFIER | ITEM_DOES_NOT_EXIST | INTERNAL_ERROR | INVALID_FINDING_ID | INVALID_SCAN_NAME). The model distinguishes an identifier whose *value* the service cannot even parse as one of its identifiers (INVALID_FINDING_ID / INVALID_SCAN_NAME) from an identifier that is syntactically fine but names nothing (ITEM_DOES_NOT_EXIST). That split is visible in the model itself: the REQUEST member `FindingIdentifier$scanName`/`$findingId` are plain unconstrained `String`s (so the service accepts anything and must classify it), while the RESPONSE member `BatchGetFindingsError$scanName` targets the constrained `ScanName` shape (length 1..140, pattern ^[a-zA-Z0-9-_$:.]*$) — i.e. the service's own notion of a name that *is* valid. A client that submits a garbage findingId (not a CodeGuru Security finding identifier at all) should therefore see INVALID_FINDING_ID, and a client that submits a perfectly well-formed scanName for a scan it has not created should see the identifier reported as not existing.

Actual: the handler branches only on store membership and so can emit exactly three of the five modelled codes, with the wrong split:
* `findingId` is never examined at all — any value, well-formed or garbage, that does not key an entry in the findings store is reported as ITEM_DOES_NOT_EXIST. INVALID_FINDING_ID is unreachable from every input, so the emulator can never tell a client that the identifier it sent is not a finding identifier.
* any `scanName` that is not a key of `store.scans` is reported as INVALID_SCAN_NAME, including a name that fully satisfies the `ScanName` constraints and merely names no scan of the account. The emulator therefore has no way to say "that identifier does not exist" for a scan it does not hold, and it uses the 'invalid name' classification for the 'unknown resource' case.
* a side effect of the same shortcut: a scanName that violates `ScanName` (e.g. 200 characters, or containing a space) is echoed verbatim into `BatchGetFindingsError$scanName`, a member whose target shape forbids that value, so the emulator emits a response member outside its modelled constraint.

This is distinct from the already-filed GetFindings issue ("No request path ever produces findings"): it concerns which `errorCode` an unresolvable identifier is labelled with, which is wrong regardless of whether the findings store can ever be populated. Because the findings store has no writer, the mislabelled failedFindings entries are in fact the ONLY output BatchGetFindings can ever produce for a client, which is what raises this above cosmetic.

## Evidence

Smithy model: `smithy_query(query="shape",shape="ErrorCode")` -> enum values DUPLICATE_IDENTIFIER, ITEM_DOES_NOT_EXIST, INTERNAL_ERROR, INVALID_FINDING_ID, INVALID_SCAN_NAME; `BatchGetFindingsError` requires scanName (target `ScanName`, length 1..140, pattern ^[a-zA-Z0-9-_$:.]*$), findingId (String), errorCode (ErrorCode), message (String); `FindingIdentifier` members are unconstrained `String`s. Handler: services/service-com-amazonaws-codegurusecurity-awscodegurusecurity/src/ops/batch_get_findings.rs lines 184-192 answer INVALID_SCAN_NAME for `!store.scans.contains_key(&identifier.scan_name)` (no name-syntax check anywhere), and lines 194-206 answer ITEM_DOES_NOT_EXIST for every unresolved findingId (no id-format check anywhere); `crate::state` defines only ERROR_CODE_INVALID_SCAN_NAME / ERROR_CODE_ITEM_DOES_NOT_EXIST / ERROR_CODE_DUPLICATE_IDENTIFIER, so INVALID_FINDING_ID appears nowhere in src/. Test coverage: tests/all/scan_lifecycle.rs lines 729-781 (`batch_get_findings_classifies_every_identifier_it_cannot_resolve`) LOCKS IN the current mapping — it asserts ITEM_DOES_NOT_EXIST for findingId "absent-finding" against a known scan and INVALID_SCAN_NAME for scanName "no-such-scan" — and its own comment scopes itself to "each ErrorCode the store can justify", so no test covers INVALID_FINDING_ID or an unknown-but-well-formed scan name being reported as non-existent.

## Steps to Reproduce

1. POST /uploadUrl {"scanName":"known-scan"} then POST /scans with the returned codeArtifactId and scanName=known-scan, so one scan exists
2. POST /batchGetFindings with {"findingIdentifiers":[{"scanName":"known-scan","findingId":"not-a-finding-identifier"}]} -> observe failedFindings[0].errorCode = ITEM_DOES_NOT_EXIST; the identifier is not a CodeGuru Security finding id at all, which is the case the modelled INVALID_FINDING_ID exists for (no input can make the emulator emit that code)
3. POST /batchGetFindings with {"findingIdentifiers":[{"scanName":"never-created-scan","findingId":"anything"}]} -> observe failedFindings[0].errorCode = INVALID_SCAN_NAME even though "never-created-scan" satisfies the ScanName length/pattern constraints and the real distinction is that nothing by that name exists
4. POST /batchGetFindings with {"findingIdentifiers":[{"scanName":"bad name with spaces","findingId":"x"}]} -> observe the response echoes that scanName into failedFindings[0].scanName, a member whose target shape (ScanName) forbids spaces
