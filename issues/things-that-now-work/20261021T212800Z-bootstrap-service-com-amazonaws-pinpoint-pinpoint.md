Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the pinpoint emulator

_Filed: 2026-10-21T21:28:00Z_
_Operation: service-com-amazonaws-pinpoint-pinpoint | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 122 Pinpoint operations is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the application (project) + endpoint lifecycle, which is the core loop every Pinpoint client starts with. All six operations use the restJson1 protocol (SDK id `Pinpoint`, sigv4 name `mobiletargeting`).

Happy path to implement, in order:

1. **CreateApp** — `POST /v1/apps` -> 201. Input: `CreateApplicationRequest` (@httpPayload) with required `Name` plus optional `tags`. Must mint a new application `Id` (12-ish char lowercase hex, unique per call), build `Arn` as `arn:aws:mobiletargeting:<region>:<account>:apps/<Id>`, record `CreationDate` (ISO 8601), store the app, and return an `ApplicationResponse{Id, Name, Arn, tags, CreationDate}` whose fields are the stored ones.
2. **GetApp** — `GET /v1/apps/{ApplicationId}` -> 200. Must look the stored app up by `ApplicationId` and return the SAME `ApplicationResponse` that CreateApp returned (same Id/Name/Arn/tags/CreationDate). Unknown id -> `NotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **GetApps** — `GET /v1/apps` -> 200, query params `page-size`, `token`. Must return `ApplicationsResponse{Item: [ApplicationResponse...], NextToken}` listing exactly the apps currently stored — created apps present, deleted apps absent. Honour `page-size` by truncating and returning an opaque `NextToken` that a follow-up call resumes from; `NextToken` must be absent/null on the last page.
4. **UpdateEndpoint** — `PUT /v1/apps/{ApplicationId}/endpoints/{EndpointId}` -> 202. Input: `ApplicationId`, `EndpointId` (@httpLabel) and `EndpointRequest` (@httpPayload: `Address`, `ChannelType`, `Attributes`, `Metrics`, `User`, `Location`, `Demographic`, `OptOut`, `EndpointStatus`, `EffectiveDate`, `RequestId`). Must create-or-merge the endpoint under that application and persist every supplied member; returns `MessageBody{Message, RequestID}`. Unknown `ApplicationId` -> `NotFoundException` 404.
5. **GetEndpoint** — `GET /v1/apps/{ApplicationId}/endpoints/{EndpointId}` -> 200. Must return `EndpointResponse` built from the persisted endpoint: `Id` = the `EndpointId` used on write, `ApplicationId` = the owning app, and `Address`, `ChannelType`, `Attributes`, `Metrics`, `User`, `Location`, `Demographic`, `OptOut`, `EndpointStatus` exactly as written, plus server-generated `CreationDate`, `EffectiveDate` and `CohortId`. Unknown endpoint -> `NotFoundException` 404.
6. **DeleteApp** — `DELETE /v1/apps/{ApplicationId}` -> 200. Must remove the app (and its endpoints) from state and return the deleted app's `ApplicationResponse`. A subsequent GetApp must return `NotFoundException` 404 and GetApps must no longer list it. Deleting an unknown id -> `NotFoundException` 404.

NONE of these may be canned, hardcoded or echo-only: each response must be computed from the shared per-service store reached via `crate::state::service_state(&*self.engine)`. An id minted without storage, or a Get that reflects its own input, is a failure of this issue. Errors must serialize as the modeled restJson1 error shapes (`NotFoundException` 404, `BadRequestException` 400) — e.g. CreateApp with a missing/empty `Name` must be `BadRequestException`, not a 200.

## State Model

In `ComAmazonawsPinpointPinpointState`:

- `apps: HashMap<String /*ApplicationId*/, App>`
- `App { id: String, name: String, arn: String, creation_date: String /*ISO8601*/, tags: HashMap<String,String>, endpoints: HashMap<String /*EndpointId*/, Endpoint> }`
- `Endpoint { id: String, application_id: String, address: Option<String>, channel_type: Option<ChannelType>, attributes: HashMap<String, Vec<String>>, metrics: HashMap<String, f64>, user: Option<EndpointUser>, location: Option<EndpointLocation>, demographic: Option<EndpointDemographic>, opt_out: Option<String>, endpoint_status: Option<String>, request_id: Option<String>, cohort_id: String, creation_date: String, effective_date: String }`
- An app-name/id counter or UUID source used only to MINT ids that are then inserted into `apps` (never returned without storing).
- Pagination: GetApps sorts app ids deterministically and encodes the resume position in `NextToken`.

Everything else (campaigns, segments, journeys, templates, channels, SendMessages, PutEvents, KPI/analytics, tagging) stays 501 for now and will be filed as individual gap issues later.

## Evidence

services/service-com-amazonaws-pinpoint-pinpoint/OPERATIONS.md declares 122 operations (restJson1, SDK id `Pinpoint`, sigv4 `mobiletargeting`) with the routes/payloads cited above: CreateApp `POST /v1/apps -> 201` (CreateApplicationRequest -> ApplicationResponse), GetApp `GET /v1/apps/{ApplicationId} -> 200`, GetApps `GET /v1/apps -> 200` with `page-size`/`token` -> ApplicationsResponse{Item,NextToken}, UpdateEndpoint `PUT /v1/apps/{ApplicationId}/endpoints/{EndpointId} -> 202` (EndpointRequest -> MessageBody), GetEndpoint `GET /v1/apps/{ApplicationId}/endpoints/{EndpointId} -> 200` -> EndpointResponse, DeleteApp `DELETE /v1/apps/{ApplicationId} -> 200` -> ApplicationResponse; all six declare `NotFoundException (HTTP 404)` and `BadRequestException (HTTP 400)`. Field lists confirmed from the Smithy model shapes ApplicationResponse (required Id/Name/Arn, plus tags, CreationDate), CreateApplicationRequest (required Name, tags), ApplicationsResponse (Item, NextToken), EndpointRequest/EndpointResponse and MessageBody(Message, RequestID). A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under services/service-com-amazonaws-pinpoint-pinpoint/src/ops/ matches every handler file, including create_app.rs, get_app.rs, get_apps.rs, update_endpoint.rs, get_endpoint.rs and delete_app.rs — so there is no state, no tests asserting any of this behavior, and all calls return 501.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK Pinpoint client at it.
2. CreateApp with `ApplicationRequest={Name: 'demo-project', tags: {env: 'test'}}` — expect 201 and an ApplicationResponse with a freshly minted Id, an Arn containing that Id, Name 'demo-project' and the tags echoed from storage. Today: HTTP 501 NotImplemented.
3. GetApp with the returned ApplicationId — expect 200 and the identical Id/Name/Arn/tags/CreationDate from step 2.
4. GetApps with no params — expect the new app in `Item`; repeat with `page-size=1` after creating a second app and verify a `NextToken` is returned and resuming with it yields the remaining app without duplicates.
5. UpdateEndpoint with ApplicationId from step 2, EndpointId='endpoint-1' and EndpointRequest={Address:'user@example.com', ChannelType:'EMAIL', Attributes:{Interests:['Music']}, Metrics:{Logins: 3}, User:{UserId:'user-1'}} — expect 202 with a MessageBody.
6. GetEndpoint for ApplicationId/'endpoint-1' — expect 200 with Id='endpoint-1', ApplicationId set, Address/ChannelType/Attributes/Metrics/User exactly as written in step 5, plus server-set CreationDate/EffectiveDate/CohortId.
7. GetEndpoint for an unknown EndpointId — expect NotFoundException with HTTP 404.
8. DeleteApp with the ApplicationId — expect 200 with the deleted app's ApplicationResponse; then GetApp returns NotFoundException 404, GetApps no longer lists it, and GetEndpoint for 'endpoint-1' returns NotFoundException 404.
