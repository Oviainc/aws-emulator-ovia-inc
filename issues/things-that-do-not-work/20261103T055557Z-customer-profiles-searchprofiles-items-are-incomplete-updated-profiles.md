Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-ccp-traj-wrong-field-then-update
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-ccp-traj-wrong-field-then-update customer-profiles-ccp-traj-create-input-mistakes
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|SearchProfiles|2xx->2xx|$.Items[*]
Issue-Key: customer-profiles|SearchProfiles|2xx->2xx|$.Items[*].PartyTypeString
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# customer-profiles SearchProfiles items are incomplete: updated profiles unfindable and PartyTypeString dropped (2 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchProfiles

## Description
Two net-new wire causes sharing ONE fault: the Profile objects SearchProfiles returns are built from an incomplete view of the stored profile - in one case the profile is missing from the result page altogether, in the other a stored member is not projected.

Cause 1 of 2: a profile that is searchable by a predefined key is absent from the result page after the write path that set that key.
AWS: Smithy: SearchProfiles (POST /domains/{DomainName}/profiles/search) returns Items as a ProfileList of `Profile` structures, each carrying ProfileId, the standard profile members (null when unset), Attributes, and FoundByItems (a list of {KeyName, Values} naming the stored key that matched). Recorded response: 200 with ONE item for the profile whose `_account` index holds the searched value - ProfileId set, Attributes carrying the account number, FoundByItems == [{KeyName: '_account', Values: [<the searched value>]}], and the unset standard members present as null.
Emulator: 200 with an EMPTY Items list - $.Items[0] is missing entirely, so the profile that was just written and indexed cannot be found by the key it is indexed under.
Why: this is a silent data-loss branch. The search succeeds (200), so a client reads 'no such customer' and will create a duplicate profile or skip an update instead of operating on the record that exists. The emulator's state holds the profile; only the search view does not.
Fix: the SearchProfiles matching path and the key index the profile write path maintains - the index entry for a key written or changed by a profile write must be visible to a subsequent search, and the matched profile must be projected into Items with FoundByItems naming the stored key.
Traces: customer-profiles-ccp-traj-wrong-field-then-update#task#7.

Cause 2 of 2: PartyTypeString is not projected into the returned Profile.
AWS: Smithy: Profile$PartyTypeString targets sensitiveString1To255 and is documented as 'An alternative to PartyType which accepts any string as input'; CreateProfileRequest carries the same member. Recorded response: 200 whose Items[0].PartyTypeString echoes the value supplied at CreateProfile (and whose PartyType stays null when only the string form was supplied).
Emulator: 200 with the item present but $.Items[0].PartyTypeString missing (got None), so a value accepted at write time is never read back.
Why: PartyTypeString exists precisely so callers can store a party type outside the deprecated PartyType enum; dropping it from the search projection makes that data write-only, and a client that round-trips profiles through search loses the field.
Fix: the SearchProfiles Profile projection (and the profile state it reads) - serialize PartyTypeString, and the other string alternatives alongside it, from the stored profile.
Traces: customer-profiles-ccp-traj-create-input-mistakes#task#8.

No open issue covers either cause: open behavioral issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md touches SearchProfiles only for FoundByItems normalization (its Cause 7) and for two missing refusal gates (its Causes 8 and 9).

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false). Both clusters are verdict=mismatch with expected_status == actual_status == 200.

Cause 1 - fixture customer-profiles-ccp-traj-wrong-field-then-update step task#7, SearchProfiles, 200 -> 200:
  $.Items[0]: missing - the recorded item is a full Profile with ProfileId set, Attributes {'AccountNumber': <value>}, AccountNumber <value>, FoundByItems [{'KeyName': '_account', 'Values': [<value>]}] and every other standard member null; the emulator returned None for Items[0] (an empty page).

Cause 2 - fixture customer-profiles-ccp-traj-create-input-mistakes step task#8, SearchProfiles, 200 -> 200:
  $.Items[0].PartyTypeString: missing (expected the value supplied at CreateProfile - 'INDIVIDUAL' in this recording - got None). The rest of the item matched, so only this member is unprojected.

The account number, profile id and the particular PartyTypeString value in the recording are that run's inputs; the requirement is that the indexed profile appears in Items with FoundByItems naming the stored key, and that PartyTypeString round-trips.

## Steps to Reproduce
Cause 1: CreateDomain(D, DefaultExpirationDays=366); CreateProfile(D, AccountNumber=A) -> P; issue the profile write that sets/changes the account key for P (the fixture's UpdateProfile step); then SearchProfiles(D, KeyName='_account', Values=[A]) -> expect 200 with exactly one item whose ProfileId == P, Attributes {'AccountNumber': A} and FoundByItems [{'KeyName':'_account','Values':[A]}]; observe 200 with an empty Items list.
Cause 2: CreateDomain(D2); CreateProfile(D2, AccountNumber=A2, PartyTypeString='INDIVIDUAL'); SearchProfiles(D2, KeyName='_account', Values=[A2]) -> expect Items[0].PartyTypeString == the value supplied; observe the member absent from the item.

Re-replay fixtures customer-profiles-ccp-traj-wrong-field-then-update and customer-profiles-ccp-traj-create-input-mistakes to confirm.
