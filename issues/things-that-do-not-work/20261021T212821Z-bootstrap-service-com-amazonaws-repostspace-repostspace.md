Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the repostspace emulator

_Filed: 2026-10-21T21:28:21Z_
_Operation: service-com-amazonaws-repostspace-repostspace | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 operations in `services/service-com-amazonaws-repostspace-repostspace/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST minimal slice: the stateful lifecycle of the primary resource, the private re:Post *space* (`restJson1`, sdkId `repostspace`).

### Happy path to implement (in order)

1. **CreateSpace** — `POST /spaces`. Input: `name` (1..30, required), `subdomain` (1..63, required), `tier` (`BASIC|STANDARD`, required), optional `description`, `userKMSKey`, `tags`, `roleArn`, `supportedEmailDomains`. Output: `spaceId` (required). Must mint a new unique `spaceId`, derive and persist the space's `arn` (`arn:aws:repostspace:<region>:<account>:space/<spaceId>`), `randomDomain` (AWS-generated subdomain), `vanityDomain` (from the requested `subdomain`), `createDateTime` (now), `status`, `configurationStatus`, `vanityDomainStatus`, `storageLimit` derived from `tier`, and the supplied `tags`. Store the record; return only the generated id.
2. **GetSpace** — `GET /spaces/{spaceId}`. Must look the stored space up by id and return exactly what CreateSpace persisted: `spaceId`, `arn`, `name`, `status`, `configurationStatus`, `clientId`, `vanityDomainStatus`, `vanityDomain`, `randomDomain`, `createDateTime`, `tier`, `storageLimit`, plus optional `description`, `customerRoleArn`, `userKMSKey`, `userAdmins`, `groupAdmins`, `roles`, `userCount`, `contentSize`, `supportedEmailDomains`. Unknown id -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListSpaces** — `GET /spaces`. Must return one `SpaceData` entry per persisted space (`spaceId`, `arn`, `name`, `status`, `configurationStatus`, `vanityDomainStatus`, `vanityDomain`, `randomDomain`, `tier`, `storageLimit`, `createDateTime`, optional `description`/`userCount`/`contentSize`), values consistent with GetSpace. Honour the `@paginated` contract: cap the page at `maxResults` (range 1..100, default 10) and emit/accept an opaque `nextToken` that resumes where the previous page stopped; omit `nextToken` on the last page.
4. **UpdateSpace** — `PUT /spaces/{spaceId}`. Input: `description`, `tier`, `roleArn`, `supportedEmailDomains` (all optional). Must mutate the stored record in place so a following GetSpace reflects the new values (and `storageLimit` follows a changed `tier`); absent members must leave the existing value untouched. Unknown id -> `ResourceNotFoundException` (404). Empty output body.
5. **DeleteSpace** — `DELETE /spaces/{spaceId}`. Must remove the record (or mark it deleted with `deleteDateTime`) so a subsequent GetSpace returns `ResourceNotFoundException` (404) and the space no longer appears in ListSpaces. Unknown id -> `ResourceNotFoundException` (404). Empty output body.

Optionally round out the slice with **CreateChannel** / **GetChannel** (`POST|GET /spaces/{spaceId}/channels[/{channelId}]`) once the space lifecycle is stateful — CreateChannel must mint a 24-character `channelId` stored under its parent space, and GetChannel must echo back the persisted `channelName`, `channelDescription`, `createDateTime` and `channelStatus`. A channel request for an unknown `spaceId` must be a 404, not a success.

### Rules

- NEVER STUBS: each of the five operations must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). Hardcoded bodies, echoing the request back, or minting an id without storing the record are failures.
- Required output members must be populated with real derived values, not empty strings.
- Validation: enforce the modeled constraints (`name` 1..30, `subdomain` 1..63, `description` 1..255, `tier` enum, `maxResults` 1..100) and reject violations with `ValidationException` (HTTP 400). A duplicate `subdomain` should raise `ConflictException` (HTTP 409).
- All state must survive across requests in the single shared engine — a create followed by a read in a separate HTTP call must agree.
- Done = a client script performing CreateSpace -> GetSpace -> ListSpaces -> UpdateSpace -> GetSpace -> DeleteSpace -> GetSpace(404) passes with correct bodies, not merely 2xx.

## State Model

- `spaces: HashMap<String /* spaceId */, Space>` — the service's single primary index.
- `Space { space_id: String, arn: String, name: String, description: Option<String>, subdomain: String, vanity_domain: String, vanity_domain_status: VanityDomainStatus, random_domain: String, status: String /* ProvisioningStatus, e.g. CREATED */, configuration_status: ConfigurationStatus, client_id: String, identity_store_id: Option<String>, application_arn: Option<String>, customer_role_arn: Option<String>, tier: TierLevel, storage_limit: i64, user_kms_key: Option<String>, create_date_time: DateTime, delete_date_time: Option<DateTime>, user_count: i32, content_size: i64, supported_email_domains: Option<SupportedEmailDomainsStatus>, user_admins: Vec<String>, group_admins: Vec<String>, roles: HashMap<String /* accessorId */, Vec<Role>>, tags: HashMap<String, String>, channels: HashMap<String /* channelId */, Channel> }`
- `Channel { channel_id: String /* exactly 24 chars */, space_id: String, channel_name: String, channel_description: Option<String>, create_date_time: DateTime, delete_date_time: Option<DateTime>, channel_status: ChannelStatus, channel_roles: HashMap<String, Vec<ChannelRole>> }`
- `arn_index: HashMap<String /* arn */, String /* spaceId */>` so the later `/tags/{resourceArn}` operations can resolve a space by ARN.
- Pagination: `nextToken` is an opaque encoding of a stable position in the id-sorted space (or channel) key list, so ListSpaces/ListChannels can resume deterministically.

The other 12-14 operations (RegisterAdmin, DeregisterAdmin, SendInvites, BatchAddRole, BatchRemoveRole, BatchAddChannelRoleToAccessors, BatchRemoveChannelRoleFromAccessors, ListChannels, UpdateChannel, TagResource, UntagResource, ListTagsForResource) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-repostspace-repostspace/OPERATIONS.md` lists all 19 operations with routes, members and errors: CreateSpace `POST /spaces` -> `spaceId`; GetSpace `GET /spaces/{spaceId}` with required `arn`/`name`/`status`/`configurationStatus`/`clientId`/`vanityDomain`/`randomDomain`/`createDateTime`/`tier`/`storageLimit`; ListSpaces `GET /spaces` `@paginated(inputToken=nextToken, outputToken=nextToken, items=spaces)` with `maxResults` range 1..100 default 10; UpdateSpace `PUT /spaces/{spaceId}`; DeleteSpace `DELETE /spaces/{spaceId}`; all declaring `ResourceNotFoundException (HTTP 404)`, `ValidationException (HTTP 400)` and (for create/update) `ConflictException (HTTP 409)`. `SpaceData` members for ListSpaces confirmed via smithy_query on the model. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 19 handler files (create_space.rs, get_space.rs, list_spaces.rs, update_space.rs, delete_space.rs, create_channel.rs, get_channel.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501, so no state is held and there is no test coverage of any behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI repostspace client at it.
2. `aws repostspace create-space --name demo --subdomain demo-co --tier STANDARD --description 'bootstrap demo' --tags team=platform` — expect a 200 with a freshly generated `spaceId`; today it returns HTTP 501 NotImplemented.
3. `aws repostspace get-space --space-id <spaceId>` — expect the persisted record back: `name=demo`, `description='bootstrap demo'`, `tier=STANDARD`, an `arn` containing the spaceId, a populated `vanityDomain`/`randomDomain`, a real `createDateTime` and a `storageLimit`.
4. `aws repostspace list-spaces --max-results 1` — expect exactly one SpaceData whose fields match get-space plus a `nextToken` when more spaces exist; pass the token back and expect the next space, with no token on the final page.
5. `aws repostspace update-space --space-id <spaceId> --description updated --tier BASIC` then `get-space` again — expect `description=updated`, `tier=BASIC`, `storageLimit` updated, and `name`/`createDateTime`/`arn` unchanged.
6. `aws repostspace create-space --name demo2 --subdomain demo-co --tier BASIC` — expect ConflictException (HTTP 409) for the duplicate subdomain.
7. `aws repostspace create-space --name '' --subdomain demo3 --tier BASIC` — expect ValidationException (HTTP 400) for the length-constraint violation.
8. `aws repostspace delete-space --space-id <spaceId>`, then `get-space --space-id <spaceId>` — expect ResourceNotFoundException (HTTP 404) and the space absent from list-spaces.
9. `aws repostspace get-space --space-id does-not-exist` — expect ResourceNotFoundException (HTTP 404), not 501.
