Issue-Severity: MEDIUM

# The Chime family's generic tagging routes are reachable only at a family endpoint host: five services bind the identical `/tags` routes and the only discriminator (ResourceARN) is in the request body

> _Filed by the fixer of `20261022T042904Z-chime-sdk-identity-is-entirely-unreachable-the-router.md`, which made every OTHER Chime route reachable. This is the one residual it could not resolve, recorded so it is not implicit._

## Operation
TagResource (and UntagResource, ListTagsForResource) of the five Chime SDK services

## Description
Five emulated services sign with the SigV4 name `chime` AND bind the very same three
generic tagging routes:

* `GET /tags?arn=<ResourceARN>` — ListTagsForResource
* `POST /tags?operation=tag-resource` — TagResource
* `POST /tags?operation=untag-resource` — UntagResource

They are Chime SDK Identity, Chime SDK Messaging, Chime SDK Meetings, Chime SDK Media
Pipelines and Chime SDK Voice (`smithy_query(query="operation",operation="TagResource")`
on each model; Amazon Chime itself binds no tagging operations). Real AWS separates the
five by the endpoint host the client was pointed at (`identity-chime`,
`messaging-chime`, `meetings-chime`, `media-pipelines-chime`, `voice-chime`).

The family's claim rule (`services/common/src/chime_family.rs`) resolves this one path
space by endpoint prefix ONLY: a request that names a family endpoint is served by that
endpoint, and a request that names none — which is what a client pointed at a local
`endpoint_url` sends — is claimed by nobody, so the router answers its terminal
`404 UnknownService`. That is deliberate (a 404 beats serving one of five claimants
arbitrarily, and beats the retryable `500 AmbiguousService` the whole family used to
answer), but it leaves these three operations unreachable for the ordinary local client:
`client.tag_resource(...)` on a `chime-sdk-identity` client pointed at the emulator gets
`404 UnknownService` rather than the modelled `204`.

Why it is not already fixed: `Service::can_handle` is handed `http::request::Parts`, not
the request body. `ListTagsForResource` carries its ARN in the `arn` query parameter
(visible), but `TagResource` / `UntagResource` bind `ResourceARN` to the BODY, which the
claim predicate cannot see. Resolving them for a request that names no endpoint therefore
needs either a body-aware claim seam in the framework (`vendor/`, read-only from a service
crate) or a declared owner for the family's shared tagging routes backed by one ARN-keyed
tag store, so that whichever member serves the call answers identically.

Why the visible discriminator is not used for `GET /tags` on its own: the three routes are
one read-write set over the same tags. Narrowing only the read would give the write
(`POST /tags?operation=tag-resource`) one claimant — whoever ends up owning it — and the
read a different one, chosen per ARN, so a tag written through the owner would not be
visible to the member that claims the read unless both already share one ARN-keyed store.
A tag that cannot be read back is a worse answer than the refusal, so the three routes
have to be decided together, by the same owner-or-shared-store choice this issue asks for.
Nothing is lost by waiting: all three operations are scaffolded `501`s in the members that
bind them, so no implemented behaviour is behind the gap today.

## Evidence
With all six Chime services mounted and a request that names no family endpoint (the local
`endpoint_url` case), no service claims `/tags`, so the router answers:

```
404 UnknownService
```

The same request addressed to `identity-chime.us-east-1.amazonaws.com` IS claimed by Chime
SDK Identity and answers that operation's scaffold, `501 NotImplemented`
(`services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/tests/all/signing_name_route_claim.rs`
pins both halves).

Expected (AWS, restJson1): `204` with an empty body for TagResource/UntagResource, and
`200 {"Tags": [...]}` for ListTagsForResource — or the service's own modelled error
(`BadRequestException`, `NotFoundException`, `ResourceLimitExceededException`).

Note that all three operations are ALSO still scaffolded (`501 NotImplemented`) in the
Chime SDK Identity, Messaging and Media Pipelines crates, so this routing gap is what a
caller hits first, not the only work needed.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. `client = session.client("chime-sdk-identity", region_name="us-east-1", endpoint_url=<emulator>)`.
3. `client.create_app_instance(Name="t", ClientRequestToken=uuid4().hex)` — succeeds (201).
4. `client.tag_resource(ResourceARN=<the AppInstanceArn>, Tags=[{"Key":"k","Value":"v"}])`.
5. Observe `404 UnknownService` instead of a 204 (or the modelled error / the scaffold 501).
6. The same holds for `untag_resource` and `list_tags_for_resource`, and for the
   equivalent calls on chime-sdk-messaging, chime-sdk-meetings, chime-sdk-media-pipelines
   and chime-sdk-voice.
