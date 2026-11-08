Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.configuration-sets-v1-and-v2-pagination-and-cross-api-token-continuation
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|ListConfigurationSets|BadRequestException
Issue-Commit: 2073bfe006ea868c0fcc9de72b8615a8a5f91eae

# ses: a v1 ListConfigurationSets NextToken is refused by SESv2 ListConfigurationSets (400 BadRequestException "Token is invalid.") instead of continuing the listing — the two surfaces mint incompatible, privately-tagged cursors

> _Note: this issue was identified against the code base at commit `2073bfe006ea868c0fcc9de72b8615a8a5f91eae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConfigurationSets

## Description
One net-new root cause, grounded in the single failing behavioral check the replay report marked neither ALREADY FILED nor OWNED. No open issue covers it: 20261028T010932Z-ses-2-net-new-behavioral-root-causes-createconfigurationseteventdestination.md owns the IDENTITY listings (SESv2 ListEmailIdentities' forged-cursor wording and the v1 ListIdentities legs), not the configuration-set listings; 20261102T110414Z-sesv2-3-net-new-refusal-message-divergences-listdedicatedippools.md owns ListDedicatedIpPools' forged-NextToken sentence, a different operation; 20261029T171854Z and 20261028T103411Z own configuration-set READ/DELETE semantics, not pagination-cursor interchange.

=== Cause 1 of 1: a NextToken issued by SES (v1) ListConfigurationSets is rejected by SESv2 ListConfigurationSets instead of resuming the configuration-set listing at that position ===

AWS: SES classic and SESv2 are two API surfaces over ONE account-level configuration-set collection, and the configuration-set listing's pagination cursor is shared between them: a NextToken handed back by the v1 `ListConfigurationSets` is accepted by the v2 `ListConfigurationSets` and continues the same walk, so the continued page does NOT repeat the item page one already reported. Smithy (ses, com.amazonaws.ses#ListConfigurationSets): input `MaxItems` + `NextToken`, output `ConfigurationSets` + `NextToken` ("A token indicating that there are additional configuration sets available to be listed. Pass this token to successive calls of ListConfigurationSets."), and the operation models NO errors at all. Smithy (sesv2, com.amazonaws.sesv2#ListConfigurationSets): POST /v2/email/list-configuration-sets, input `Filter` + `NextToken` + `PageSize`, paginated inputToken/outputToken `NextToken`, errors `BadRequestException` (httpError 400, "The input you provided is invalid.") and `TooManyRequestsException` (429); `NextToken` is documented only as "A token returned from a previous call to ListConfigurationSets to indicate the position in the list of configuration sets" — the model does NOT scope that previous call to one of the two surfaces. Neither model states a per-surface cursor namespace, so the interchange is recorded behavior only: the recording shows the v2 call carrying the v1 token answering HTTP 200 with a page that omits page one's set.

Emulator: the v2 call is REFUSED. `sesv2.ListConfigurationSets(NextToken=<token the v1 ListConfigurationSets just returned>)` answers HTTP 400 with error code `BadRequestException` and message `Token is invalid.` (botocore: "An error occurred (BadRequestException) when calling the ListConfigurationSets operation: Token is invalid."). The two surfaces mint cursors in private, mutually unreadable formats, so neither can ever read the other's: the ses crate encodes hex(integrity-tag ‖ key) and decodes it under the listing name `"ListConfigurationSets"`, while the sesv2 crate encodes base64("sesv2:configuration-sets:" ‖ key) and decodes only under that tag — so the v1 token fails the v2 tag check, `decode_tagged_page_token` returns None, and the handler raises the crate's generic unissued-cursor refusal. The semantics also differ and would still disagree after a shared format: the v1 token names the FIRST set of the next page and v1 resumes inclusively at it, while the v2 token names the LAST set of the page just served and v2 resumes strictly after it — so a cursor handed across would be off by one even once it decoded. Note the two surfaces' own round trips are fine: the v1 MaxItems=1 walk and the v2 PageSize=1 walk both completed with one item per page, no duplicates and no dropped sets, so only the CROSS-surface continuation diverges.

Why: a client that pages the account's configuration sets while migrating from SES to SESv2 — or any SDK/tool that holds a cursor and issues the next page through whichever client it has — gets a hard 400 on a cursor the service itself just issued, where AWS serves the next page. Because the refusal is `BadRequestException` "Token is invalid.", a caller cannot tell a genuinely corrupt or expired cursor from a perfectly valid one it is simply handing to the other surface, so the usual recovery (restart the walk from the beginning) is the only option and any mid-walk hand-off is impossible. It also makes the emulator's cursor an API-surface-private value where AWS's is an account-listing position, which breaks migration and dual-client parity tests outright.

Fix: the configuration-set page-cursor codec shared by the two ListConfigurationSets handlers — `crate::state::encode_page_token`/`decode_page_token` with `LISTING_CONFIGURATION_SETS` on the classic side and `crate::state::encode_tagged_page_token`/`decode_tagged_page_token` with `CONFIGURATION_SETS_PAGE_TOKEN_TAG` on the v2 side, as used by the two `ops/list_configuration_sets.rs` handlers. The configuration-set listing cursor must live in ONE place both surfaces read and write (alongside the account-wide configuration-set collection, the way the identity directory is already shared), with one agreed resume semantics, so a cursor either surface issues names a position in the account's listing rather than a position private to the API that minted it. An unissued or malformed cursor must still be refused on each surface with that surface's own modelled error and wording.

Traces: ses.configuration-sets-v1-and-v2-pagination-and-cross-api-token-continuation

## Evidence
Check ses.configuration-sets-v1-and-v2-pagination-and-cross-api-token-continuation (verdict: error):
  botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the ListConfigurationSets operation: Token is invalid.
  raised from the cross-API continuation leg, `continued = v2.list_configuration_sets(NextToken=token)` where `token` is the NextToken returned by `v1.list_configuration_sets(MaxItems=1)` immediately before.

  Expected (AWS): HTTP 200 from SESv2 ListConfigurationSets, a `ConfigurationSets` page that continues the walk, and page one's single set name absent from it (the check's assertion: "v1's NextToken continued into a v2 page that repeats page one's item").
  Actual (emulator): HTTP 400, Code=BadRequestException, Message="Token is invalid." — the whole sentence is fixed emulator text (`crate::state::invalid_next_token()`), carrying no part of the request; the rejected token is not echoed.

  Legs that PASSED before it, so the defect is narrowly the cross-surface hand-off and not pagination in general:
   - three configuration sets created through sesv2.create_configuration_set;
   - v1 walk at MaxItems=1 to exhaustion: every page had <= 1 item, no duplicate names, none of the three created sets dropped;
   - v2 walk at PageSize=1 to exhaustion: every page had <= 1 item, no duplicate names, none of the three created sets dropped;
   - v1.list_configuration_sets(MaxItems=1) returned exactly one item AND a non-empty NextToken (so the token under test is one the emulator itself minted and considers valid on its own surface).

  Cursor formats that make the hand-off impossible (why the refusal is structural, not a one-off):
   - ses crate: encode_page_token(listing, key) = hex(page_token_tag(listing, key) ‖ key); decode_page_token requires the tag recomputed for listing "ListConfigurationSets" to match.
   - sesv2 crate: encode_tagged_page_token(tag, last) = base64(tag ‖ last) with tag "sesv2:configuration-sets:"; decode_tagged_page_token requires that exact prefix after base64-decoding.
  A hex v1 token therefore never carries the v2 prefix, and the v2 handler's `.ok_or_else(|| bad_request(invalid_next_token()))` fires.

## Steps to Reproduce
1. Create three configuration sets so the listing has more than one page at page size 1: sesv2.create_configuration_set(ConfigurationSetName=<name-i>) for i in 0..3.
2. Walk v1 to exhaustion: ses.list_configuration_sets(MaxItems=1), then repeat with NextToken until none is returned. Expect one item per page, no duplicates, all three names present. (This passes today.)
3. Walk v2 to exhaustion: sesv2.list_configuration_sets(PageSize=1), then repeat with NextToken until none is returned. Same expectations. (This passes today.)
4. Take a fresh v1 cursor: first = ses.list_configuration_sets(MaxItems=1); token = first["NextToken"] (non-empty), first_name = first["ConfigurationSets"][0]["Name"].
5. Hand that cursor to the other surface: sesv2.list_configuration_sets(NextToken=token).
   Expected: HTTP 200, a ConfigurationSets page continuing the account listing, with first_name NOT in it.
   Actual: ClientError 400, Code=BadRequestException, Message="Token is invalid."
6. The symmetric direction is broken the same way and should be checked with the same fix: take a NextToken from sesv2.list_configuration_sets(PageSize=1) and pass it as ses.list_configuration_sets(NextToken=...); the classic handler decodes under its own hex tag only and refuses it (InvalidParameterValue "Invalid PageToken <...>.").
