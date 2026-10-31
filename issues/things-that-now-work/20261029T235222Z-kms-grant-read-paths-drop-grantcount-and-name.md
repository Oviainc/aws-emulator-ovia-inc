Issue-Source: trace-sweep-divergence
Issue-Trace: kms-grant-create-list-revoke
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-grant-create-list-revoke kms-grant-create-list-revoke-lifecycle kms-grant-dry-run-does-not-create kms-grant-lifecycle-create-list-retire-revoke kms-grant-lifecycle-management kms-grant-multiple-grantees-isolation kms-grant-name-constraints-retire-by-grantid kms-grant-retire-by-token kms-grant-retiring-principal-and-retire-by-token kms-grant-service-principal-and-retire-by-ids kms-grant-service-principal-with-constraints kms-list-retirable-grants-for-principal kms-multiple-grants-isolation kms-list-retirable-grants-invalid-marker-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|CreateGrant|2xx->2xx|$.KeyOrigin
Issue-Key: kms|ListGrants|2xx->2xx|$.GrantCount
Issue-Key: kms|ListGrants|2xx->2xx|$.Grants[Name="sim2real-probe-kms-grant-alpha-266345ee"].Operations[*]
Issue-Key: kms|ListGrants|2xx->2xx|$.Grants[*]
Issue-Key: kms|ListRetirableGrants|2xx->2xx|$.GrantCount
Issue-Key: kms|ListRetirableGrants|2xx->2xx|$.Grants[*]
Issue-Key: kms|ListGrants|2xx->2xx|$.Grants[*].Name
Issue-Key: kms|ListGrants|2xx->2xx|$.Grants[Name="sim2real-probe-kms-grant-svc-bf73de0d"].Operations[*]
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# KMS grant read paths drop GrantCount and Name, and order grants and grant operations by insertion instead of canonically

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGrants

## Description
Every grant-plane read in the corpus diverges from the recording in the same three ways, plus CreateGrant omits the same key-origin projection the rest of the KMS surface omits. The grant records themselves are stored correctly (GrantId, KeyId, principals and Constraints all match) — what is wrong is the PROJECTION: a count member is never emitted, an unnamed grant's Name is dropped instead of being reported as the empty string, the Operations list is echoed in request order instead of the canonical order, and the Grants list is emitted in creation order instead of the canonical order. 10 causes.

Smithy facts used (services/service-com-amazonaws-kms-trentservice/smithy-model): ListGrantsResponse models Grants (GrantList), NextMarker and Truncated ONLY — there is no modeled GrantCount, so that member is recorded behavior only. ListRetirableGrants shares output_shape ListGrantsResponse. GrantListEntry models Name (GrantNameType, length 1..256) and documents it as 'If a name was provided in the CreateGrant request, that name is returned. Otherwise this value is null' — but the RECORDED wire reports Name as the empty string '' for a grant created without a Name, so the recording is the requirement here. GrantOperationList is a plain list of the GrantOperation enum, so the model does not fix its order; the recording does, and it is alphabetical. CreateGrantResponse models GrantToken and GrantId only, so KeyOrigin there is recorded behavior only.

Cause 1 of 10: CreateGrant omits KeyOrigin.
AWS: recorded behavior only — the 200 CreateGrant body carries KeyOrigin, the Origin of the key the grant was created on ('AWS_KMS' in the recording; the value comes from the key's stored state, not a constant).
Emulator: 200 with $.KeyOrigin missing (expected 'AWS_KMS', got None). This is the single widest cluster in the corpus: 18 affected steps across 13 grant fixtures.
Why: a client reading the grant-creation response cannot tell whether the grant is on an AWS_KMS or an EXTERNAL-origin key.
Fix: the CreateGrant handler's response serializer.
Traces: kms-grant-create-list-revoke#task#2.

Cause 2 of 10: ListGrants never emits GrantCount, and sorts Operations by insertion.
AWS: recorded behavior only for GrantCount — the recorded ListGrants 200 body carries GrantCount equal to the number of entries in Grants (2 in this recording). For Operations the recorded order is ascending alphabetical by enum value ('Decrypt' then 'Encrypt'), independent of the order the operations were listed in CreateGrant.
Emulator: 200 with $.GrantCount missing (expected 2, got None) and Operations echoed in request order — $.Grants[Name=...].Operations[0] value_mismatch (expected 'Decrypt', got 'Encrypt'), Operations[1] (expected 'Encrypt', got 'Decrypt').
Why: a client that pages on GrantCount sees no count; a client that compares an expected permission set to Operations by position (or hashes the serialized list to detect grant drift) gets a false difference on every grant.
Fix: the ListGrants handler's response projection — emit the count, and canonicalize the Operations list on the way out (or sort it at CreateGrant time in state).
Traces: kms-grant-create-list-revoke#task#4.

Cause 3 of 10: ListGrants omits GrantCount on a single-grant key.
AWS: recorded behavior only — GrantCount present, equal to len(Grants).
Emulator: 200 with $.GrantCount missing (expected 1, got None). 13 affected steps.
Fix: the ListGrants response projection.
Traces: kms-grant-create-list-revoke#task#5.

Cause 4 of 10: ListGrants drops Name for a grant created without one, and mis-orders Operations.
AWS: the recorded GrantListEntry for a grant created with no Name carries Name: '' (empty string), and Operations alphabetical ('Decrypt','Encrypt'). GrantCount is present.
Emulator: the entry matches on KeyId/GrantId/GranteePrincipal/IssuingAccount but has NO Name member at all and Operations in request order ('Encrypt','Decrypt'); GrantCount missing.
Why: a client that reads entry.Name to key its own grant bookkeeping gets a missing field rather than the empty string, which in strongly typed SDKs is a different value (null vs "").
Fix: the ListGrants entry serializer — always render Name, defaulting to the empty string for an unnamed grant.
Traces: kms-grant-create-list-revoke-lifecycle#task#3 (4 affected steps).

Cause 5 of 10: ListRetirableGrants omits GrantCount when there is nothing to retire.
AWS: recorded behavior only — the 200 body carries GrantCount: 0 alongside an empty Grants list.
Emulator: 200 with $.GrantCount missing (expected 0, got None). Note this is the empty case, so a client cannot even fall back to len(Grants) semantics being reported.
Fix: the ListRetirableGrants response projection.
Traces: kms-grant-lifecycle-create-list-retire-revoke#task#6 (3 affected steps).

Cause 6 of 10: ListGrants returns multiple grants in creation order rather than the canonical order.
AWS: with two grants on one key the recorded Grants list is ordered by GrantId ascending (GrantId '8c571e2d...' before 'e6f7957b...'), and GrantCount is present.
Emulator: returns them in creation order — Grants[0] carries GrantId 'e6f7957b...' where the recording has '8c571e2d...' and vice versa for Grants[1] — plus GrantCount missing, Name dropped and Operations in request order.
Why: list order is part of what a client diffs between polls; an insertion-ordered list makes a steady-state grant set look like it changed, and makes pagination over Limit/Marker non-deterministic against AWS.
Fix: the ListGrants handler's state query — order the result set by GrantId before projecting.
Traces: kms-grant-lifecycle-management#task#5 (2 affected steps).

Cause 7 of 10: ListRetirableGrants has the same ordering and projection defects as cause 6.
AWS: the recorded body orders Grants by GrantId ascending, carries GrantCount, and renders Name as '' plus alphabetical Operations on each entry.
Emulator: creation order, no GrantCount, no Name, request-order Operations.
Fix: the ListRetirableGrants handler's state query and entry serializer (same projection code path as ListGrants — they share output_shape ListGrantsResponse).
Traces: kms-grant-lifecycle-management#task#6.

Cause 8 of 10: ListGrants after a retire-by-token flow drops Name and GrantCount.
AWS: the surviving grant is reported with Name: '' and the body carries GrantCount: 1.
Emulator: 200 with $.GrantCount missing (expected 1, got None) and $.Grants[0].Name missing (expected '', got None).
Fix: the ListGrants entry serializer / response projection.
Traces: kms-grant-retire-by-token#task#3.

Cause 9 of 10: ListRetirableGrants drops GrantCount and Name for a grant carrying Constraints.
AWS: the recorded entry carries Constraints.EncryptionContextEquals, Name: '', alphabetical Operations, and the body carries GrantCount: 1.
Emulator: the Constraints and Operations happen to match here, but GrantCount is missing and the entry has no Name member.
Fix: the shared grant-entry serializer.
Traces: kms-grant-retiring-principal-and-retire-by-token#task#5 (2 affected steps).

Cause 10 of 10: ListGrants mis-orders Operations on a service-principal grant and drops GrantCount.
AWS: Operations alphabetical — 'Decrypt' then 'GenerateDataKey' — and GrantCount present.
Emulator: Operations in request order ('GenerateDataKey','Decrypt') and GrantCount missing. This confirms the ordering defect is a plain echo of the request list, not a fixed two-element swap.
Fix: canonicalize GrantOperationList ordering in the grant projection.
Traces: kms-grant-service-principal-with-constraints#task#3.

Related open issue, which does NOT cover any of the above (it is about over-permissive grant state and GrantId derivation, not the read projection): 20261029T175759Z-kms-5-net-new-over-permissive-state-root.md.

## Evidence
Replay of simparity-wire-aws-v1, 10 mismatch clusters, all expected_status 200 / actual_status 200:
- CreateGrant, kms-grant-create-list-revoke#task#2: $.KeyOrigin: missing (expected 'AWS_KMS', got None); 18 affected steps.
- ListGrants, kms-grant-create-list-revoke#task#4: $.GrantCount: missing (expected 2, got None); Operations[0] expected 'Decrypt' got 'Encrypt'; Operations[1] expected 'Encrypt' got 'Decrypt'.
- ListGrants, kms-grant-create-list-revoke#task#5: $.GrantCount: missing (expected 1, got None); 13 affected steps.
- ListGrants, kms-grant-create-list-revoke-lifecycle#task#3: $.GrantCount missing (expected 1); $.Grants[0] expected to carry Name: '' and Operations ['Decrypt','Encrypt'], actual entry has no Name member and Operations ['Encrypt','Decrypt'] (same GrantId); 4 affected steps.
- ListRetirableGrants, kms-grant-lifecycle-create-list-retire-revoke#task#6: $.GrantCount: missing (expected 0, got None); 3 affected steps.
- ListGrants, kms-grant-lifecycle-management#task#5: $.GrantCount missing (expected 2); $.Grants[0] expected GrantId '8c571e2d...' got 'e6f7957b...', $.Grants[1] expected 'e6f7957b...' got '8c571e2d...' — i.e. creation order instead of GrantId-ascending; 2 affected steps.
- ListRetirableGrants, kms-grant-lifecycle-management#task#6: same ordering/projection difference on the retirable list.
- ListGrants, kms-grant-retire-by-token#task#3: $.GrantCount missing (expected 1); $.Grants[0].Name missing (expected '').
- ListRetirableGrants, kms-grant-retiring-principal-and-retire-by-token#task#5: $.GrantCount missing (expected 1); $.Grants[0] expected with Name: '' present; 2 affected steps.
- ListGrants, kms-grant-service-principal-with-constraints#task#3: $.GrantCount missing (expected 1); Operations[0] expected 'Decrypt' got 'GenerateDataKey'; Operations[1] expected 'GenerateDataKey' got 'Decrypt'.
Smithy cross-check: GrantCount is not modeled on ListGrantsResponse (recorded behavior only); Name IS modeled on GrantListEntry; GrantOperationList order is unconstrained by the model and alphabetical in the recording.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Inspect kms-grant-create-list-revoke step task#2: CreateGrant answers 200 without KeyOrigin.
4. Inspect step task#4 of the same fixture: ListGrants answers 200 with no GrantCount and with Operations in the order they were supplied to CreateGrant rather than alphabetical.
5. Inspect kms-grant-create-list-revoke-lifecycle#task#3 and kms-grant-retire-by-token#task#3: the returned GrantListEntry for a grant created with no Name has no Name member, where the recording carries Name: ''.
6. Inspect kms-grant-lifecycle-management#task#5 and #task#6: with two grants on one key the list comes back in creation order, so Grants[0]/Grants[1] GrantIds are swapped relative to the GrantId-ascending recording.
7. Inspect kms-grant-lifecycle-create-list-retire-revoke#task#6: ListRetirableGrants with nothing retirable omits GrantCount: 0.
8. Equivalent by hand: CreateKey; CreateGrant twice with Operations supplied as [Encrypt, Decrypt] and no Name; then ListGrants and ListRetirableGrants and diff against the recorded bodies.
