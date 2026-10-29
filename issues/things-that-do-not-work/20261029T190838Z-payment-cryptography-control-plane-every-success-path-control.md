Issue-Source: trace-sweep-divergence
Issue-Trace: payment-cryptography-alias-management-v2
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: payment-cryptography-alias-management-v2 payment-cryptography-asymmetric-key-csr-and-certificate-v2 payment-cryptography-export-import-tr31-roundtrip-v2 payment-cryptography-key-lifecycle-symmetric-v2 payment-cryptography-key-replication-regions-v2 payment-cryptography-resource-policy-workflow-v2 payment-cryptography-default-key-replication-regions-v2 payment-cryptography-export-import-parameters-v2
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|CreateKey|2xx->5xx|$.Key
Issue-Key: payment-cryptography|CreateKey|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|CreateKey|2xx->5xx|status
Issue-Key: payment-cryptography|GetAlias|2xx->5xx|$.Alias
Issue-Key: payment-cryptography|GetAlias|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetAlias|2xx->5xx|status
Issue-Key: payment-cryptography|UpdateAlias|2xx->5xx|$.Alias
Issue-Key: payment-cryptography|UpdateAlias|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|UpdateAlias|2xx->5xx|status
Issue-Key: payment-cryptography|DeleteAlias|2xx->5xx|$
Issue-Key: payment-cryptography|DeleteAlias|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|DeleteAlias|2xx->5xx|status
Issue-Key: payment-cryptography|GetDefaultKeyReplicationRegions|2xx->5xx|$.EnabledReplicationRegions
Issue-Key: payment-cryptography|GetDefaultKeyReplicationRegions|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetDefaultKeyReplicationRegions|2xx->5xx|status
Issue-Key: payment-cryptography|EnableDefaultKeyReplicationRegions|2xx->5xx|$.EnabledReplicationRegions
Issue-Key: payment-cryptography|EnableDefaultKeyReplicationRegions|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|EnableDefaultKeyReplicationRegions|2xx->5xx|status
Issue-Key: payment-cryptography|DisableDefaultKeyReplicationRegions|2xx->5xx|$.EnabledReplicationRegions
Issue-Key: payment-cryptography|DisableDefaultKeyReplicationRegions|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|DisableDefaultKeyReplicationRegions|2xx->5xx|status
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|$.ExportToken
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|$.ParametersValidUntilTimestamp
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|$.SigningKeyAlgorithm
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|$.SigningKeyCertificate
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|$.SigningKeyCertificateChain
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetParametersForExport|2xx->5xx|status
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|$.ImportToken
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|$.ParametersValidUntilTimestamp
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|$.WrappingKeyAlgorithm
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|$.WrappingKeyCertificate
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|$.WrappingKeyCertificateChain
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|GetParametersForImport|2xx->5xx|status
Issue-Key: payment-cryptography|ListKeys|2xx->5xx|$.Keys
Issue-Key: payment-cryptography|ListKeys|2xx->5xx|$headers.content-type
Issue-Key: payment-cryptography|ListKeys|2xx->5xx|status
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# Payment Cryptography control plane: every success-path control operation refused with AmbiguousService 500

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKey

## Description
This bundle has 10 causes, numbered below. Every one of them is a success-path Payment Cryptography CONTROL-PLANE call that the emulator never dispatches: the router refuses the request pre-handler with HTTP 500 and an emulator-authored error code 'AmbiguousService' ("N emulated services all claim requests for '<route-key>': <claimants>; the request is rejected rather than served by an arbitrary one"), and frames the response as application/x-amz-json-1.1 instead of the awsJson1_0 content type. Because the refusal happens before dispatch, no required output member is ever produced. A single routing fix is likely to clear all ten, but each cause names a distinct operation whose 200 response and required output members must be observed before it is closed.

Shared protocol defect present in every cause: the control plane is awsJson1_0 (Smithy: service PaymentCryptographyControlPlane, protocols [awsJson1_0], sdkId "Payment Cryptography"), so every response - success or error - must carry Content-Type: application/x-amz-json-1.0. The emulator answers application/x-amz-json-1.1 on all ten.

Related open issue: 20261029T174910Z-payment-cryptography-control-plane-is-entirely-unreachable-the.md files the same routing root cause from the BEHAVIORAL corpus. It carries no wire replay key, so it does not cover these wire fixture steps; it does identify the seam (can_handle comparing only the shared sigv4Name 'payment-cryptography', which both the control-plane and data-plane models declare).

Cause 1 of 10: CreateKey returns no key.
AWS: Smithy CreateKey -> CreateKeyOutput with required member Key (a Key structure carrying KeyArn, KeyAttributes {KeyAlgorithm, KeyClass, KeyModesOfUse, KeyUsage}, KeyCheckValue, KeyCheckValueAlgorithm, KeyOrigin AWS_PAYMENT_CRYPTOGRAPHY, KeyState CREATE_COMPLETE, Enabled, Exportable, CreateTimestamp, UsageStartTimestamp). Recorded response: HTTP 200, body {"Key": {...}}, Content-Type application/x-amz-json-1.0. The ARN, key-check value and timestamps are per-run values, not requirements; the required shape is.
Emulator: HTTP 500, $.Key missing, content-type application/x-amz-json-1.1, body carries __type 'AmbiguousService'.
Why: a client cannot create a key, so nothing downstream in the service is usable; and because 'AmbiguousService' is not in CreateKey's modeled error union (ConflictException, ServiceQuotaExceededException, ServiceUnavailableException + common AccessDeniedException, InternalServerException, ResourceNotFoundException, ThrottlingException, ValidationException), SDK error handling never matches and a 500 is treated as retryable.
Fix: the control-plane crate's request-claim seam (can_handle) so exactly one service claims an awsJson1_0 'payment-cryptography' request, then CreateKey's handler/serializer.
Traces: payment-cryptography-alias-management-v2#setup#1 (this cluster also covers 7 more CreateKey setup steps across payment-cryptography-asymmetric-key-csr-and-certificate-v2, payment-cryptography-export-import-tr31-roundtrip-v2, payment-cryptography-key-lifecycle-symmetric-v2, payment-cryptography-key-replication-regions-v2, payment-cryptography-resource-policy-workflow-v2).

Cause 2 of 10: GetAlias on an existing alias returns no alias.
AWS: Smithy GetAlias -> GetAliasOutput with required member Alias {AliasName, KeyArn}. Recorded: HTTP 200, {"Alias": {"AliasName": "alias/...", "KeyArn": "<arn of the bound key>"}}.
Emulator: HTTP 500, $.Alias missing, 'AmbiguousService', content-type 1.1.
Why: alias resolution is how callers address keys by friendly name; a client cannot read back an alias it just created.
Fix: routing claim seam, then GetAlias's handler reading alias state.
Traces: payment-cryptography-alias-management-v2#task#3.

Cause 3 of 10: UpdateAlias does not re-point the alias.
AWS: Smithy UpdateAlias -> UpdateAliasOutput with Alias {AliasName, KeyArn}; KeyArn is absent/null when the alias is detached from its key. Recorded: HTTP 200 with the updated Alias.
Emulator: HTTP 500, $.Alias missing, 'AmbiguousService', content-type 1.1.
Why: a caller cannot move an alias between keys, which is the documented way to cut over consumers.
Fix: routing claim seam, then UpdateAlias's handler/state mutation.
Traces: payment-cryptography-alias-management-v2#task#5.

Cause 4 of 10: DeleteAlias does not delete.
AWS: Smithy DeleteAlias -> DeleteAliasOutput, no members. Recorded: HTTP 200 with an empty JSON body {}.
Emulator: HTTP 500 with a non-empty body whose __type is 'AmbiguousService' and message is the emulator's claimants text; content-type 1.1.
Why: aliases accumulate and the alias namespace cannot be reclaimed; a client that branches on a 200/empty-body acknowledgement fails.
Fix: routing claim seam, then DeleteAlias's handler and its empty-body serializer.
Traces: payment-cryptography-alias-management-v2#task#8.

Cause 5 of 10: GetDefaultKeyReplicationRegions returns no region list.
AWS: Smithy GetDefaultKeyReplicationRegionsOutput with required member EnabledReplicationRegions (list of Regions); an account with none enabled returns an empty list, not a missing member. Recorded: HTTP 200, {"EnabledReplicationRegions": []}.
Emulator: HTTP 500, member missing, 'AmbiguousService', content-type 1.1.
Why: a client cannot read the account's default replication configuration, and cannot distinguish "none enabled" from "call failed".
Fix: routing claim seam, then the default-replication-region state read.
Traces: payment-cryptography-default-key-replication-regions-v2#task#1 (cluster also covers task#3 and task#5 of the same fixture).

Cause 6 of 10: EnableDefaultKeyReplicationRegions does not enable.
AWS: Smithy EnableDefaultKeyReplicationRegionsOutput with required EnabledReplicationRegions echoing the post-change set. Recorded: HTTP 200 with the enabled region present in the list.
Emulator: HTTP 500, member missing, 'AmbiguousService', content-type 1.1.
Why: default replication cannot be turned on, and the subsequent Get reads an unchanged account.
Fix: routing claim seam, then the enable handler's state write.
Traces: payment-cryptography-default-key-replication-regions-v2#task#2.

Cause 7 of 10: DisableDefaultKeyReplicationRegions does not disable.
AWS: Smithy DisableDefaultKeyReplicationRegionsOutput with required EnabledReplicationRegions echoing the post-change set (empty once the last region is removed). Recorded: HTTP 200, {"EnabledReplicationRegions": []}.
Emulator: HTTP 500, member missing, 'AmbiguousService', content-type 1.1.
Why: the enable/disable round trip cannot complete, so replication state is permanently unobservable.
Fix: routing claim seam, then the disable handler's state write.
Traces: payment-cryptography-default-key-replication-regions-v2#task#4.

Cause 8 of 10: GetParametersForExport issues no export token.
AWS: Smithy GetParametersForExportOutput requires SigningKeyCertificate, SigningKeyCertificateChain, SigningKeyAlgorithm, ExportToken (pattern ^(export-token-[0-9a-zA-Z]{16,64})?$) and ParametersValidUntilTimestamp. Recorded: HTTP 200 with SigningKeyAlgorithm "RSA_2048" and shaped (per-run) values for the certificates, token and expiry.
Emulator: HTTP 500, all five members missing, 'AmbiguousService', content-type 1.1.
Why: TR-34 export cannot be initiated at all, so ExportKey is unreachable for any client following the documented two-step flow.
Fix: routing claim seam, then the export-parameter handler that mints and stores the token.
Traces: payment-cryptography-export-import-parameters-v2#task#1 (cluster also covers task#2).

Cause 9 of 10: GetParametersForImport issues no import token.
AWS: Smithy GetParametersForImportOutput requires WrappingKeyCertificate, WrappingKeyCertificateChain, WrappingKeyAlgorithm, ImportToken (pattern ^(import-token-[0-9a-zA-Z]{16,64})?$) and ParametersValidUntilTimestamp. Recorded: HTTP 200 with WrappingKeyAlgorithm "RSA_2048" and shaped per-run values for the rest.
Emulator: HTTP 500, all five members missing, 'AmbiguousService', content-type 1.1.
Why: key import cannot be initiated, so ImportKey is unreachable.
Fix: routing claim seam, then the import-parameter handler that mints and stores the token.
Traces: payment-cryptography-export-import-parameters-v2#task#3 (cluster also covers task#4).

Cause 10 of 10: ListKeys returns no key list.
AWS: Smithy ListKeysOutput with required Keys (KeySummaryList of KeyArn, KeyAttributes, KeyCheckValue, KeyState, Enabled, Exportable) plus optional NextToken. Recorded: HTTP 200 with the created key present as a summary in CREATE_COMPLETE state.
Emulator: HTTP 500, $.Keys missing, 'AmbiguousService', content-type 1.1.
Why: a client cannot enumerate keys or filter them by state, so no inventory or state-transition check works.
Fix: routing claim seam, then ListKeys' handler reading key state.
Traces: payment-cryptography-key-lifecycle-symmetric-v2#task#3.

## Evidence
Replay of simparity-wire-aws-v1: 11 fixtures replayed, 0/73 steps passed, 31 mismatched, 42 blocked, score 0%. All 10 causes in this bundle report the same actual response.

Expected (recorded AWS, control-plane endpoint, awsJson1_0, Content-Type application/x-amz-json-1.0):
  CreateKey                           -> 200, {"Key": {...KeyArn, KeyAttributes, KeyCheckValue, KeyOrigin: AWS_PAYMENT_CRYPTOGRAPHY, KeyState: CREATE_COMPLETE...}}
  GetAlias / UpdateAlias              -> 200, {"Alias": {"AliasName": "alias/...", "KeyArn": ...}}
  DeleteAlias                         -> 200, {}
  GetDefaultKeyReplicationRegions     -> 200, {"EnabledReplicationRegions": []}
  EnableDefaultKeyReplicationRegions  -> 200, {"EnabledReplicationRegions": [<enabled region>]}
  DisableDefaultKeyReplicationRegions -> 200, {"EnabledReplicationRegions": []}
  GetParametersForExport              -> 200, {"SigningKeyCertificate", "SigningKeyCertificateChain", "SigningKeyAlgorithm": "RSA_2048", "ExportToken", "ParametersValidUntilTimestamp"}
  GetParametersForImport              -> 200, {"WrappingKeyCertificate", "WrappingKeyCertificateChain", "WrappingKeyAlgorithm": "RSA_2048", "ImportToken", "ParametersValidUntilTimestamp"}
  ListKeys                            -> 200, {"Keys": [<KeySummary>]}

Actual (emulator), identical for all ten:
  HTTP 500, Content-Type: application/x-amz-json-1.1
  body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'payment-cryptography': payment-cryptography, payment-cryptography; the request is rejected rather than served by an arbitrary one"}
  Every required output member is absent (reported by replay as "$.Key: missing", "$.Alias: missing", "$.EnabledReplicationRegions: missing", "$.ExportToken: missing", "$.ImportToken: missing", "$.Keys: missing").

Message anatomy: the whole 'AmbiguousService' string is emulator framework text with no AWS counterpart - its fixed part is "<N> emulated services all claim requests for '<route-key>': <claimants>; the request is rejected rather than served by an arbitrary one" and its variable parts (N, route key, claimant list) come from the emulator's own service registry, not from the request. No part of it is a requirement; the requirement is that the request be routed and served. 'AmbiguousService' is not a member of any modeled error union in com.amazonaws.paymentcryptography, and 500 is not a modeled status for any of these operations' success paths.

Content-type evidence: replay reports "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.0', got 'application/x-amz-json-1.1')" on 9 of the 10 causes (the tenth asserted status only). Smithy digest for this model: protocols [awsJson1_0].

## Steps to Reproduce
1. Boot the emulator from this checkout (both the paymentcryptography control-plane crate and the paymentcryptographydata data-plane crate are workspace members, and both declare sigv4Name 'payment-cryptography').
2. Replay fixture payment-cryptography-alias-management-v2, step setup#1 (CreateKey). Observe HTTP 500 with __type 'AmbiguousService' and Content-Type application/x-amz-json-1.1 instead of HTTP 200 with a required Key member and Content-Type application/x-amz-json-1.0.
3. Replay the other nine causes, each of which must independently return 200 with its required output members: payment-cryptography-alias-management-v2#task#3 (GetAlias), #task#5 (UpdateAlias), #task#8 (DeleteAlias, empty body); payment-cryptography-default-key-replication-regions-v2#task#1 (GetDefaultKeyReplicationRegions), #task#2 (EnableDefaultKeyReplicationRegions), #task#4 (DisableDefaultKeyReplicationRegions); payment-cryptography-export-import-parameters-v2#task#1 (GetParametersForExport), #task#3 (GetParametersForImport); payment-cryptography-key-lifecycle-symmetric-v2#task#3 (ListKeys).
4. Confirm on each that Content-Type is application/x-amz-json-1.0 and that no response body carries the 'AmbiguousService' code.
5. Note that 42 further steps in these fixtures are currently blocked behind the failing setup steps; they become replayable only once CreateKey succeeds.
