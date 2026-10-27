Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e290f8d6b034b90409323db1641d26f5c9d1b54

# DeleteResourceSet removes a set a stored policy names in ResourceSetIds, leaving that policy un-editable

_Filed: 2026-10-27T20:01:33Z_
_Operation: DeleteResourceSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e290f8d6b034b90409323db1641d26f5c9d1b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate enforces a referential rule on one side and breaks it on the other.

`PutPolicy` refuses any policy document whose `ResourceSetIds` names an id that does not resolve to a stored resource set *at the moment of the call* (`validate_resource_set_ids`). `DeleteResourceSet` removes the resource-set record with no look at `store.policies`, so a resource set a stored policy references can be deleted. The policy keeps the now-dangling id in its stored document (the document is stored verbatim and `GetPolicy` reports it back), and from then on the ordinary read-modify-write of that UNTOUCHED policy — `GetPolicy` then `PutPolicy` with the document it returned — is refused with `InvalidInputException` ("ResourceSetIds names an identifier no resource set exists for: <id>"). The policy becomes uneditable until the caller hand-edits a member it never set.

Either the delete has to account for the policies that reference the set, or the write-time rule cannot claim that every `ResourceSetIds` entry "must resolve to a resource set this organisation holds right now". As it stands the two paths contradict each other, and the store can be driven into a state the writer declares invalid.

## Evidence

The write-time rule, `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_policy.rs:407-411` (doc on `validate_resource_set_ids`):

    /// The one rule on a policy document that needs the STORE: `ResourceSetIds` are
    /// "the unique identifiers of the resource sets used by the policy"
    /// (`Policy$ResourceSetIds`), so each one must resolve to a resource set this
    /// organisation holds right now; an id that resolves to nothing is invalid
    /// input, not a policy scoped to a set that might appear later.

and the check itself, `put_policy.rs:422-441`:

    let unknown: Vec<&str> = ids
        .iter()
        .filter(|id| !store.resource_sets.contains_key(id.as_str()))
    ...
        return Err(PutPolicyErrorModeled::InvalidInputException(
            InvalidInputException { message: Some(format!(
                "ResourceSetIds names {} no resource set exists for: {}", ...

This check runs on the UPDATE branch too — `put_policy.rs:523-526` calls `validate_policy_document` and `validate_resource_set_ids` before `requested_id` is resolved: "Refused before the store is even reached ... on an UPDATE as much as on a create: both write the same `Policy`."

The other side, `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/delete_resource_set.rs:198-211` — nothing consults `store.policies`:

    // Removing the record removes the set's document, its tags and the
    // resources associated with it in one step, because the record owns all
    // three. An identifier that names no stored resource set has nothing to
    // delete, which is the modelled "the specified resource was not found".
    let Some(removed) = store.resource_sets.remove(&input.identifier) else {

The crate's own test states the consequence of the delete for the id, `services/service-com-amazonaws-fms-awsfms-20180101/tests/all/policy_document_validation.rs:570-571`:

    // The check reads the live store, not a list captured once: deleting a set
    // makes its id unacceptable.

That test only deletes a set no stored policy references (it creates the policy carrying the deleted id afterwards, `policy_document_validation.rs:583-596`), so the "delete a set a policy already uses" case is pinned by no test: `tests/all/resource_set_deletion.rs` never mentions a policy (grep for `policy|Policy` matches only the harness import and a description string).

## Steps to Reproduce

1. AssociateAdminAccount with the calling account so the administrator gate passes.
2. PutResourceSet with a Name and ResourceTypeList; note the minted Id R.
3. PutPolicy with a valid document whose `ResourceSetIds` is [R]; note the minted PolicyId P and PolicyUpdateToken T.
4. DeleteResourceSet with Identifier R — answers 200.
5. GetPolicy with PolicyId P — answers 200 and the document still carries `ResourceSetIds: [R]`.
6. PutPolicy with exactly that document (PolicyId P, PolicyUpdateToken T), changing nothing — answers 400 InvalidInputException "ResourceSetIds names an identifier no resource set exists for: R", so the policy can no longer be updated at all.
