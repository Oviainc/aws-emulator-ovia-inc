Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5d001f011ed28660c188b78afabc22e4b6c273f2

# ListSharedEndpoints drops the model's "shared by RAM" qualifier on the Outpost, so it answers for Outposts the caller owns

_Filed: 2026-10-29T06:54:13Z_
_Operation: ListSharedEndpoints | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5d001f011ed28660c188b78afabc22e4b6c273f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model scopes this operation to an Outpost the caller reaches through Resource Access Manager: "Lists all endpoints associated with an Outpost that has been shared by Amazon Web Services Resource Access Manager (RAM)", and its output member repeats the qualifier ("The list of endpoints associated with the specified Outpost that have been shared by Amazon Web Services Resource Access Manager (RAM)").

The handler applies NO such qualifier. It resolves the named `OutpostId` through `crate::state::resolve_outpost`, which accepts ANY Outpost the inventory holds in this Region — the ones the calling account owns included — and then pages every endpoint stored against it. There is no comparison anywhere in the operation between the resolved `OutpostRecord::owner_id` and `crate::state::caller_account()`, even though the crate stores exactly that distinction and uses it elsewhere. The consequence is that for an Outpost the caller owns, `ListSharedEndpoints` answers 200 with precisely the endpoints that Outpost's own records carry — i.e. it degenerates into a per-Outpost `ListEndpoints` for a resource the model says it does not serve.

That the model distinguishes the two cases, and says so when it means both, is visible one operation over: `ListOutpostsWithS3` is documented as covering "S3 on Outposts that you have access to as the Outposts owner, OR as a shared user from Resource Access Manager (RAM)", and `Outpost$OwnerId` is documented as "Useful for comparing owned versus shared outposts". `ListSharedEndpoints` names only the shared case.

The handler's own comment is where the qualifier is lost: it paraphrases the model as documenting "all endpoints associated with an Outpost" — the sentence clipped exactly before "that has been shared by ... (RAM)" — and the test suite that pins the behaviour repeats that same clipped quote as its justification. This is the misquote, not a deliberate divergence: no comment, constant or test in the crate states a reason for serving an owned Outpost here.

This is NOT the gap of the fixed issue 20261024T054024Z ("ListSharedEndpoints hides the caller's own endpoints on a shared Outpost"). That one is about WHICH ENDPOINTS on a shared Outpost are reported, and reporting all of them stays correct. This one is about WHICH OUTPOSTS the operation serves at all. `ResourceNotFoundException` (404) is modelled for the operation, and the shared-vs-owned fact is already on the record `resolve_outpost` hands back, so no new state is needed to tell the two apart.

## Evidence

MODEL (smithy_query(query="doc", shape="ListSharedEndpoints")), also carried verbatim in the handler's generated contract header — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_shared_endpoints.rs:43-45:
    //! Lists all endpoints associated with an Outpost that has been shared by Amazon Web
    //! Services Resource Access Manager (RAM). Related actions include: CreateEndpoint
    //! DeleteEndpoint

and the output member, src/ops/list_shared_endpoints.rs:59-60:
    //! * `Endpoints` (Endpoints) — The list of endpoints associated with the specified Outpost
    //!   that have been shared by Amazon Web Services Resource Access Manager (RAM).

The model SAYS "both" where it means both — src/ops/list_outposts_with_s3.rs:43-45:
    //! Lists the Outposts with S3 on Outposts capacity for your Amazon Web Services account.
    //! Includes S3 on Outposts that you have access to as the Outposts owner, or as a shared
    //! user from Resource Access Manager (RAM).
and `Outpost$OwnerId` (smithy_query(query="shape", shape="Outpost")): "Returns the Amazon Web Services account ID of the outpost owner. Useful for comparing owned versus shared outposts."

HANDLER — src/ops/list_shared_endpoints.rs:176-198, which resolves ANY Outpost of the Region and filters endpoints by Outpost id alone, with the clipped quote as its stated reason:
    // The Outpost the request named, as this Region reaches it: an Outpost
    // installed in another Region is not one this Region's endpoint serves,
    // so it resolves to the modelled 404 like one that does not exist.
    crate::state::resolve_outpost(&inventory, &region, &input.outpost_id)
        .map_err(ListSharedEndpointsErrorModeled::ResourceNotFoundException)?;
    ...
    // ... the model states
    // `OutpostId` as the only scoping member and documents "all endpoints
    // associated with an Outpost", so every endpoint on it is reported ...
    let page = crate::state::endpoint_page(
        ... |endpoint| endpoint.outpost_id == input.outpost_id,
    );
Note the handler never reads the caller account at all (unlike src/ops/list_endpoints.rs:155 and src/ops/delete_endpoint.rs:174, which both call `crate::state::caller_account()`).

The resolution helper accepts owned and shared alike — src/state.rs:807-811:
    inventory
        .get(outpost_id)
        .filter(|outpost| outpost.region == region)
        .ok_or_else(not_found)

The crate HAS the distinction it does not apply — src/state.rs:253-255:
    /// The 12-digit account that owns the Outpost — the caller's own account for
    /// an owned Outpost, another account for one shared with the caller.
    pub owner_id: String,
beside src/state.rs:827-829 (`caller_account()`).

TEST COVERAGE that pins the behaviour, on the same clipped quote — tests/all/shared_endpoint_listing.rs:5-11:
    //! The rule under test: the listing is scoped by the Outpost its `OutpostId` names and by
    //! nothing else — `OutpostId` is the only scoping member the model declares, and the
    //! operation documents "all endpoints associated with an Outpost" ...
and its rows, tests/all/shared_endpoint_listing.rs:60-62:
    ("an owned Outpost with no endpoint at all", Which::FirstOwned, 0usize),
    ("an owned Outpost with one endpoint of this account", Which::FirstOwned, 1),
    ("an owned Outpost with three endpoints of this account", Which::SecondOwned, 3),
The stated reason is the model sentence with "that has been shared by Amazon Web Services Resource Access Manager (RAM)" removed; no test or comment in the crate gives any other reason for serving an Outpost the caller owns here.

## Steps to Reproduce

1. Start the emulator with an Outpost the CALLING account owns and nothing shared: S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248" (the owner field defaults to the account this emulator serves — src/state.rs:181-184), or stand up an owned Outpost through the co-hosted outposts API
2. GET /S3Outposts/ListOutpostsWithS3?maxResults=100 -> the Outpost is reported with OwnerId == the calling account (000000000000), i.e. an Outpost the caller OWNS, not one shared with it
3. POST /S3Outposts/CreateEndpoint {"OutpostId":"op-0a1b2c3d4e5f60718","SubnetId":"subnet-0a1b2c3d4e5f60718","SecurityGroupId":"sg-0a1b2c3d4e5f60718"} -> 200 with an EndpointArn
4. GET /S3Outposts/ListSharedEndpoints?outpostId=op-0a1b2c3d4e5f60718&maxResults=100
5. Observed: 200 with the endpoint listed — the same answer ListEndpoints gives for that Outpost. The operation applied no shared-by-RAM qualifier: it never compares the resolved OutpostRecord::owner_id with caller_account()
6. Expected per the model sentence quoted above (and the output member's own wording): an Outpost the calling account owns has not "been shared by Amazon Web Services Resource Access Manager (RAM)", so it is not an Outpost this operation reports endpoints for (ResourceNotFoundException, 404, is the error the operation models for an OutpostId it does not serve)
