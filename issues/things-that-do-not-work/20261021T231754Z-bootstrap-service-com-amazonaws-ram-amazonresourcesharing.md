Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the RAM (AWS Resource Access Manager) emulator

_Filed: 2026-10-21T23:17:54Z_
_Operation: service-com-amazonaws-ram-amazonresourcesharing | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 35 operations in `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no RAM client workflow can run. Implement a FIRST, SMALL stateful slice around the primary resource (the resource share) so a realistic client script works end-to-end. Protocol: `restJson1`, sigv4 signing name `ram` (see OPERATIONS.md 'Wire protocol').

### Happy path to implement (in order)
1. **CreateResourceShare** — `POST /createresourceshare`. Input: `name` (required), `resourceArns`, `principals`, `tags`, `allowExternalPrincipals`, `permissionArns`, `clientToken`. Must mint a real `resourceShareArn` (`arn:aws:ram:<region>:<account>:resource-share/<uuid>`), store the share, and return `resourceShare` (ResourceShare) populated from the stored record: `resourceShareArn`, `name`, `owningAccountId`, `allowExternalPrincipals` (default true when absent), `status = ACTIVE`, `tags`, `creationTime`, `lastUpdatedTime`, `featureSet = STANDARD`. Also materialise one ResourceShareAssociation per entry in `resourceArns` (associationType `RESOURCE`) and per entry in `principals` (associationType `PRINCIPAL`), each with `status = ASSOCIATED`, `resourceShareName`, `creationTime`/`lastUpdatedTime`. Echo `clientToken` back, and make a repeat call with the same `clientToken` idempotent.
2. **GetResourceShares** — `POST /getresourceshares`. Input: `resourceOwner` (required, SELF | OTHER-ACCOUNTS), optional `resourceShareArns`, `resourceShareStatus`, `name`, `tagFilters`, `maxResults` (1..500), `nextToken`. Must return the shares actually created (filtered by the supplied arns/name/status; `resourceOwner = SELF` returns locally created shares, `OTHER-ACCOUNTS` returns none in the emulator), honouring `maxResults` and emitting/accepting `nextToken` for the remainder. Deleted shares must not appear when `resourceShareStatus` is unset or ACTIVE.
3. **AssociateResourceShare** — `POST /associateresourceshare`. Input: `resourceShareArn` (required), `resourceArns`, `principals`, `sources`, `clientToken`. Must append new associations to the stored share and return `resourceShareAssociations` describing exactly the entities just associated (`status = ASSOCIATED`). Unknown `resourceShareArn` → `UnknownResourceException` (HTTP 400).
4. **GetResourceShareAssociations** — `POST /getresourceshareassociations`. Input: `associationType` (required, PRINCIPAL | RESOURCE | SOURCE), optional `resourceShareArns`, `resourceArn`, `principal`, `associationStatus`, `maxResults`, `nextToken`. Must read the associations persisted by steps 1 and 3 and filter by type/share/status — never return a canned list.
5. **UpdateResourceShare** — `POST /updateresourceshare`. Input: `resourceShareArn` (required), `name`, `allowExternalPrincipals`, `clientToken`. Must mutate the stored share, bump `lastUpdatedTime`, and return the updated ResourceShare; a subsequent GetResourceShares must show the new `name`/`allowExternalPrincipals`. Unknown arn → `UnknownResourceException`.
6. **DeleteResourceShare** — `DELETE /deleteresourceshare?resourceShareArn=...&clientToken=...`. Must remove/tombstone the share (`status = DELETED`), drop its associations, return `returnValue = true` plus the `clientToken`; a second delete or a delete of an unknown arn → `UnknownResourceException` (HTTP 400), and the share must no longer be returned by GetResourceShares.

Every one of these must compute its response from persisted state — no canned bodies, no echoing the request back, no minting an ARN that is never stored. A create followed by a read must return what was created; a delete followed by a read must show the share gone.

## State Model
Store in the service state (`src/state.rs`, reached via `crate::state::service_state(&*self.engine)`):

- `resource_shares: HashMap<String /* resourceShareArn */, ResourceShareRecord>` where
  `ResourceShareRecord { arn: String, name: String, owning_account_id: String, allow_external_principals: bool, status: ResourceShareStatus /* ACTIVE|DELETING|DELETED */, status_message: Option<String>, tags: Vec<Tag{key,value}>, creation_time: SystemTime, last_updated_time: SystemTime, feature_set: ResourceShareFeatureSet /* STANDARD */, permission_arns: Vec<String>, resource_share_configuration: Option<ResourceShareConfiguration> }`
- `associations: Vec<AssociationRecord>` (or `HashMap<String, Vec<AssociationRecord>>` keyed by share arn) where
  `AssociationRecord { resource_share_arn: String, resource_share_name: String, associated_entity: String /* resource ARN, principal id, or source id */, association_type: PRINCIPAL|RESOURCE|SOURCE, status: ResourceShareAssociationStatus /* ASSOCIATED|DISASSOCIATED */, status_message: Option<String>, creation_time: SystemTime, last_updated_time: SystemTime, external: bool }`
- `client_tokens: HashMap<String /* clientToken */, String /* resourceShareArn or cached response id */>` so repeated `clientToken` values are idempotent and a reused token with different parameters yields `IdempotentParameterMismatchException`.
- `next_tokens: HashMap<String /* nextToken */, usize /* offset */>` (or an opaque encoded offset) shared by the paginated list operations so `maxResults`/`nextToken` are real and an unrecognised token yields `InvalidNextTokenException`.

Done = an integration script can CreateResourceShare → GetResourceShares (sees it) → AssociateResourceShare → GetResourceShareAssociations (sees the new resource/principal) → UpdateResourceShare (name change visible on re-read) → DeleteResourceShare → GetResourceShares (gone) with correct bodies, not merely 2xx.

All other operations (permissions lifecycle, invitations, Tag/UntagResource, ListResources, ListPrincipals, policy promotion, EnableSharingWithAwsOrganization, …) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-ram-amazonresourcesharing/OPERATIONS.md` lists all 35 operations with the input/output members cited above (CreateResourceShare §61, DeleteResourceShare §82, GetResourceShareAssociations §125, GetResourceShares §141, UpdateResourceShare §278) and the restJson1 wire protocol. `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 35 handler files (e.g. src/ops/create_resource_share.rs:4, src/ops/get_resource_shares.rs:4, src/ops/delete_resource_share.rs:4), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. The ResourceShare and ResourceShareAssociation member lists come from the Smithy model shapes `com.amazonaws.ram#ResourceShare` and `com.amazonaws.ram#ResourceShareAssociation`; the error names/statuses (UnknownResourceException 400, IdempotentParameterMismatchException 400, InvalidNextTokenException 400) are from the modeled error lists of these operations. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `ram` client at it.
2. `aws ram create-resource-share --name demo-share --resource-arns arn:aws:ec2:us-east-1:111122223333:subnet/subnet-123 --principals 444455556666` → currently HTTP 501 NotImplemented; expected 200 with a `resourceShare` carrying a minted `resourceShareArn`, `status=ACTIVE`, the supplied name and creation timestamps.
3. `aws ram get-resource-shares --resource-owner SELF` → currently 501; expected the share created in step 2 with identical arn/name/tags.
4. `aws ram associate-resource-share --resource-share-arn <arn> --principals 777788889999` → currently 501; expected 200 with a PRINCIPAL association in ASSOCIATED status.
5. `aws ram get-resource-share-associations --association-type PRINCIPAL --resource-share-arns <arn>` → currently 501; expected both principals from steps 2 and 4.
6. `aws ram update-resource-share --resource-share-arn <arn> --name demo-share-renamed` then re-run step 3 → currently 501; expected the renamed share with a newer `lastUpdatedTime`.
7. `aws ram delete-resource-share --resource-share-arn <arn>` then re-run step 3 → currently 501; expected `returnValue=true` and the share absent, with a second delete returning UnknownResourceException (HTTP 400).
