Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.e2e-identity-population-delete-merge-unlink-final-state
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|DescribeIdentity|ResourceNotFoundException
Issue-Commit: 7da7036bd0c3fb1b6531d1fc1180304bed725812

# cognito-identity: MergeDeveloperIdentities DELETES the absorbed identity record instead of leaving it stored as a disabled identity, so DescribeIdentity on it answers 404 ResourceNotFoundException and ListIdentities(HideDisabled=false) never reports it

> _Note: this issue was identified against the code base at commit `7da7036bd0c3fb1b6531d1fc1180304bed725812`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MergeDeveloperIdentities

## Description
Cause 1 of 1: MergeDeveloperIdentities destroys the merged-away identity record rather than retaining it as a DISABLED identity of the pool. Failing check: cognito-identity.e2e-identity-population-delete-merge-unlink-final-state.

AWS: The Smithy model for MergeDeveloperIdentities documents only that "Cognito associates the given source user (SourceUserIdentifier) with the IdentityId of the DestinationUserIdentifier" — i.e. the two developer users become one user reached through ONE surviving IdentityId. The model says NOTHING about removing the other identity record, so the fate of that record is recorded behavior only. The recording shows that after the merge the absorbed identity (the one of {source, destination} whose IdentityId is not the merge's answer) still EXISTS: DescribeIdentity on it succeeds with HTTP 200 and an IdentityDescription whose IdentityId equals the requested id (Smithy: DescribeIdentity -> IdentityDescription{IdentityId, Logins, CreationDate, LastModifiedDate}); it merely carries no developer login any more, because that login moved onto the survivor. Consistently with that, the absorbed identity is DISABLED: it is hidden from ListIdentities(HideDisabled=true) but still reported by ListIdentities(HideDisabled=false) (Smithy: ListIdentities.HideDisabled, boolean, default false, "allows you to hide disabled identities"), and LookupDeveloperIdentity on it is refused with NotAuthorizedException (HTTP 403) "You do not have access to this identity" because it is no longer a developer identity. Only DeleteIdentities actually removes an identity record — after DeleteIdentities, DescribeIdentity on the deleted id is correctly refused with ResourceNotFoundException (HTTP 404) and the message "Identity '<IdentityId>' not found." (fixed text "Identity '" ... "' not found.", with the IdentityId taken verbatim from the request).

Emulator: the merge treats the absorbed identity as deleted. state.rs merge_identity_into removes it from the identity store and from the pool's identity list outright (self.identities.remove(merged_away) and identities_by_pool retain(|id| id != merged_away)), and the handler's own doc comment states the intent: "merged_away is no longer an identity of the pool". Consequently DescribeIdentity on the absorbed identity answers the DELETED-identity refusal — HTTP 404 ResourceNotFoundException, message "Identity 'us-west-2:31e425fc-074b-4db1-8267-ee470cec32d2' not found." in the replay (the id is the request's IdentityId; the fixed text is correct, it is the 404 itself that is wrong here) — instead of HTTP 200 with the identity's description. The same deletion also makes ListIdentities(HideDisabled=false) omit the absorbed identity, where AWS still lists it. Note the store already models exactly the state AWS leaves: Identity.disabled + Identity::is_disabled() ("the record survives, reachable through no login at all") with ListIdentities filtering on it and projected_logins() returning the disabled-login sentinel — the merge simply does not use it.

This is NOT a regression of the resolved stubs 20261024T145006Z-cognito-identity-mergedeveloperidentities-stub.md or 20261024T145006Z-cognito-identity-describeidentity-stub.md: both operations are implemented and DescribeIdentity's 404 wording is exactly AWS's; the divergence is the state the merge leaves behind, so DescribeIdentity is refusing a record that should still be there.

Why: a client that merges two developer users loses all addressability of the absorbed identity. Any id the application had already handed out, cached, stored in its own user table, or embedded in audit records becomes a hard 404 on DescribeIdentity instead of resolving to a still-existing disabled identity, and a full enumeration with HideDisabled=false silently under-reports the pool's population — so reconciliation jobs conclude the identity never existed, which is the same observation DeleteIdentities produces. The two distinct outcomes AWS offers (merged-away-but-retained vs. truly deleted) collapse into one.

Fix: the MergeDeveloperIdentities state transition — crate::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState::merge_identity_into, called from the handler's merge(). It must move the merged-away identity's logins onto the survivor and then leave the merged-away identity STORED with its logins emptied and disabled = true (the existing Identity::disabled / is_disabled() representation that unlink already produces), keeping it in identities and in identities_by_pool, and stamping its last_modified_date, instead of removing it from both. Revoking the absorbed identity's outstanding sessions and OpenID Connect tokens is correct and should stay. DescribeIdentity, ListIdentities and LookupDeveloperIdentity then answer correctly with no change of their own.

Traces: cognito-identity.e2e-identity-population-delete-merge-unlink-final-state (DescribeIdentity on the absorbed identity, immediately after MergeDeveloperIdentities).

## Evidence
Replay of cognito-identity.e2e-identity-population-delete-merge-unlink-final-state:

  [error] botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeIdentity operation: Identity 'us-west-2:31e425fc-074b-4db1-8267-ee470cec32d2' not found.

The check creates a pool with a DeveloperProviderName, mints two developer identities (src_user, dst_user) via GetOpenIdTokenForDeveloperIdentity, calls MergeDeveloperIdentities, takes survivor_id from the response (the emulator answers the SOURCE identity, which the check accepts) and then calls DescribeIdentity on the OTHER one:

  absorbed_id = id_src if survivor_id == id_dst else id_dst
  absorbed_identity = cognito.describe_identity(IdentityId=absorbed_id)   # expects HTTP 200
  assert absorbed_identity["IdentityId"] == absorbed_id

Expected (recorded AWS): HTTP 200, IdentityDescription with IdentityId == absorbed_id, carrying no developer login.
Actual (emulator): HTTP 404 ResourceNotFoundException, message "Identity '<absorbed_id>' not found." — the record is gone.

The SAME check shows the 404 is correct for a genuinely deleted identity, i.e. the error class and its wording are implemented right and only the state is wrong:

  cognito.delete_identities(IdentityIdsToDelete=[id_to_delete])
  err == ("ResourceNotFoundException", f"Identity '{id_to_delete}' not found.")   # passes

Source evidence in the checkout:
  services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/state.rs:1348-1358 — merge_identity_into relinks the logins, then `self.identities.remove(merged_away)` and `identities.retain(|identity_id| identity_id != merged_away)` on identities_by_pool.
  services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/merge_developer_identities.rs:384-394 — the handler's comment "the destination identity is one no more", calling merge_identity_into.
  services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/state.rs:995-1021 — Identity.disabled / is_disabled() / projected_logins(): the retained-but-disabled representation that already exists for unlink.
  services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/list_identities.rs:264-281 — ListIdentities already filters on is_disabled() for HideDisabled, so a retained disabled record would be hidden from HideDisabled=true and reported for HideDisabled=false, as AWS does.

Later assertions of the same check that this deletion also breaks (never reached, because the DescribeIdentity call raises first):
  all_listed = cognito.list_identities(IdentityPoolId=pool_id, MaxResults=60, HideDisabled=False)
  assert absorbed_id in all_ids

## Steps to Reproduce
1. Boot the emulator and create an identity pool with AllowUnauthenticatedIdentities=true and a DeveloperProviderName, e.g. CreateIdentityPool(IdentityPoolName=<name>, AllowUnauthenticatedIdentities=true, DeveloperProviderName=<dp>).
2. Mint two developer identities in that pool:
   id_src = GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<dp>: "dev-src"}).IdentityId
   id_dst = GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<dp>: "dev-dst"}).IdentityId
3. MergeDeveloperIdentities(IdentityPoolId=<pool>, DeveloperProviderName=<dp>, SourceUserIdentifier="dev-src", DestinationUserIdentifier="dev-dst") and read survivor = response.IdentityId; let absorbed be whichever of id_src/id_dst is not the survivor (the emulator answers id_src, so absorbed is id_dst).
4. Call DescribeIdentity(IdentityId=absorbed).
   Observed: HTTP 404 ResourceNotFoundException "Identity '<absorbed>' not found."
   Expected: HTTP 200 with IdentityId == absorbed and no developer login on it.
5. Call ListIdentities(IdentityPoolId=<pool>, MaxResults=60, HideDisabled=false).
   Observed: absorbed is absent.
   Expected: absorbed is present (and absent from the same call with HideDisabled=true).
6. Replay cognito-identity.e2e-identity-population-delete-merge-unlink-final-state to see the whole sequence fail at step 4.
