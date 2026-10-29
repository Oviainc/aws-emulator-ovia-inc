Issue-Source: trace-sweep-divergence
Issue-Trace: cogid-merge-getid-blank-absorbed-by-registered-developer-login
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cogid-merge-getid-blank-absorbed-by-registered-developer-login cogid-merge-idp-proven-identity-login-conflicts cognito-identity-developer-authenticated-identity-flow cogid-merge-losing-identity-disabled-after-merge cogid-merge-same-public-provider-different-users-conflict
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|ListIdentities|2xx->2xx|$.Identities[*]
Issue-Key: cognito-identity|ListIdentities|2xx->2xx|$.Identities[*].IdentityId
Issue-Key: cognito-identity|ListIdentities|2xx->2xx|$.Identities[*].Logins[*]
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|2xx->4xx|$.IdentityId
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|2xx->4xx|$.Token
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|2xx->4xx|status
Issue-Key: cognito-identity|DescribeIdentity|2xx->2xx|$.Logins[*]
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: identity merge and login-list state is not maintained

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListIdentities

## Description
This issue bundles 9 root causes with one shared fault: the emulator does not maintain the identity/login bookkeeping that AWS Cognito Identity keeps when identities are linked, merged, disabled and unlinked. AWS (recorded behavior; the model only says IdentityDescription.Logins is a list of provider names and ListIdentitiesResponse.Identities is a list of IdentityDescription): a developer login presented for an existing identity is ABSORBED into that identity rather than creating a new one; after a merge the losing identity is retained but marked, so DescribeIdentity returns the pool's region token and 'DISABLED' among its Logins; an unlinked identity's Logins collapse to 'DISABLED'; and ListIdentities reflects exactly that set, with the same number of entries and the same per-identity Logins the single-identity reads report. Emulator: it creates an extra identity instead of absorbing, never records the 'DISABLED'/region markers, and drops provider names from Logins, so the list and the per-identity views disagree with AWS and with each other. Why: a client that enumerates identities after a merge/unlink sees a different population and different provider sets — a merge appears not to have happened, or an extra guest identity appears — so authorization decisions keyed on identity count or Logins are wrong. Fix: the identity/login state model shared by GetId, GetOpenIdTokenForDeveloperIdentity, MergeDeveloperIdentities, UnlinkIdentity/UnlinkDeveloperIdentity and the DescribeIdentity/ListIdentities projections.

Cause 1 of 9: ListIdentities after a blank GetId identity is absorbed by a registered developer login. AWS: the pool holds the absorbed set — the first entry is the absorbing identity and its Logins include the developer provider name. Emulator: an extra identity is present (one more entry than AWS), the ids are shifted, and the first entry's Logins lack the developer provider. Fix: absorb instead of create in the developer-login path; ListIdentities projection. Traces: cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#12.

Cause 2 of 9: GetOpenIdTokenForDeveloperIdentity presenting an already-registered developer user for a blank identity. AWS: 200 with the absorbing IdentityId and a Token. Emulator: 400 — the call is refused, so the absorption never happens. Fix: the GetOpenIdTokenForDeveloperIdentity handler's identity-resolution path. Traces: cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#9 (3 steps).

Cause 3 of 9: DescribeIdentity on the identity that lost the merge. AWS: Logins contains 'DISABLED' and the pool's region token. Emulator: those entries are absent. Fix: record the disabled marker in identity state; DescribeIdentity projection. Traces: cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#10 (2 steps).

Cause 4 of 9: ListIdentities after an IdP-proven identity login conflict. AWS: the surviving identity's Logins include the developer provider and the pool holds no extra identity. Emulator: the provider is missing from Logins and an extra identity entry is present. Fix: as cause 1. Traces: cogid-merge-idp-proven-identity-login-conflicts#task#19.

Cause 5 of 9: DescribeIdentity on the losing identity after MergeDeveloperIdentities. AWS: Logins includes the pool's region token alongside the provider. Emulator: that entry is absent. Fix: as cause 3. Traces: cogid-merge-losing-identity-disabled-after-merge#task#6 (3 steps).

Cause 6 of 9: ListIdentities after the merge. AWS: the winning identity's Logins include the developer provider name. Emulator: that provider is missing. Fix: as cause 1. Traces: cogid-merge-losing-identity-disabled-after-merge#task#10.

Cause 7 of 9: ListIdentities including the disabled loser. AWS: the winner carries the developer provider and the loser carries the region token in Logins. Emulator: both entries are missing. Fix: as causes 1 and 3. Traces: cogid-merge-losing-identity-disabled-after-merge#task#11.

Cause 8 of 9: ListIdentities where one identity has the same developer provider linked twice (two different users of one public provider). AWS: the entry's Logins list carries the provider name once per link. Emulator: the duplicate link is collapsed to a single entry. Fix: the login-link set semantics in identity state. Traces: cogid-merge-same-public-provider-different-users-conflict#task#19.

Cause 9 of 9: DescribeIdentity after the identity's logins were unlinked. AWS: Logins[0] is 'DISABLED'. Emulator: Logins[0] is still the provider name ('login.example.otherapp'), i.e. the unlink left the login in place. Fix: UnlinkIdentity/UnlinkDeveloperIdentity must replace the login set with the disabled marker. Traces: cognito-identity-developer-authenticated-identity-flow#task#10.

Note: the per-run identity ids and epoch timestamps in the evidence below are environment values, not requirements; the requirements are the number of entries, which identity holds which provider names, and the presence of the 'DISABLED'/region markers.

## Evidence
Replay of simparity-wire-aws-v1, all clusters status-matched (200 except cause 2):
- ListIdentities: $.Identities[0].IdentityId shifted, $.Identities[0].Logins[0] missing (expected the developer provider 'login.sim2real.mergeprobe'), $.Identities[2] extra (an additional identity the recording does not have) (cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#12).
- GetOpenIdTokenForDeveloperIdentity: expected 200 with $.IdentityId and $.Token, got 400 (cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#9, 3 steps).
- DescribeIdentity: $.Logins[0] missing (expected 'DISABLED'), $.Logins[1] missing (expected the region token 'us-west-2') (cogid-merge-getid-blank-absorbed-by-registered-developer-login#task#10, 2 steps).
- ListIdentities: $.Identities[1].Logins[2] missing (expected 'login.sim2real.mergeprobe'), $.Identities[2] extra (cogid-merge-idp-proven-identity-login-conflicts#task#19).
- DescribeIdentity: $.Logins[1] missing (expected 'us-west-2') (cogid-merge-losing-identity-disabled-after-merge#task#6, 3 steps).
- ListIdentities: $.Identities[0].Logins[1] missing (expected 'login.sim2real.mergeprobe') (cogid-merge-losing-identity-disabled-after-merge#task#10).
- ListIdentities: $.Identities[0].Logins[1] and $.Identities[1].Logins[1] missing (cogid-merge-losing-identity-disabled-after-merge#task#11).
- ListIdentities: $.Identities[0] expected Logins ['cognito-idp.<region>.amazonaws.com/<user-pool-id>', 'login.sim2real.mergeprobe', 'login.sim2real.mergeprobe'], got the same list with the duplicate dropped (cogid-merge-same-public-provider-different-users-conflict#task#19).
- DescribeIdentity: $.Logins[0] expected 'DISABLED', got 'login.example.otherapp' (cognito-identity-developer-authenticated-identity-flow#task#10).
Smithy: IdentityDescription.Logins = LoginsList (list of provider-name strings); ListIdentitiesResponse.Identities = list of IdentityDescription. The DISABLED/region markers are recorded behavior only.

## Steps to Reproduce
1. Replay cogid-merge-getid-blank-absorbed-by-registered-developer-login: at task#9 GetOpenIdTokenForDeveloperIdentity for an already-registered developer user with a blank GetId identity is refused with 400 (AWS returns 200 and absorbs); at task#10 DescribeIdentity lacks the DISABLED/region Logins entries; at task#12 ListIdentities has one identity too many.
2. Replay cogid-merge-losing-identity-disabled-after-merge steps task#6, task#10, task#11 and cogid-merge-idp-proven-identity-login-conflicts step task#19 and compare Logins of each listed identity.
3. Replay cogid-merge-same-public-provider-different-users-conflict#task#19 for the duplicate-provider link, and cognito-identity-developer-authenticated-identity-flow#task#10 for the post-unlink DISABLED marker.
