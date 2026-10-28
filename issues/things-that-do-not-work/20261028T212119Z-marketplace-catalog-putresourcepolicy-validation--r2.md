Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# Resource-policy operations accept a change-set ARN, though ResourceArn is documented as the entity resource

_Filed: 2026-10-28T21:21:19Z_
_Operation: PutResourcePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three resource-policy operations are documented for an ENTITY: `PutResourcePolicy` "Attaches a resource-based policy to an entity", and its `ResourceArn` is "The Amazon Resource Name (ARN) of the entity resource you want to associate with a resource policy". The model draws the distinction explicitly elsewhere: `ListTagsForResource` is documented for "a resource (either an entity or change set)" and its `ResourceArn` is just "the resource you want to tag". This crate collapses both kinds into one `CatalogResource` parse — justified in its own doc by citing the TAGGING operation — and then reuses it for the policy operations, so a CHANGE SET ARN is accepted as a policy target: `PutResourcePolicy` answers 200 and persists a policy under the key `ChangeSet/<id>`, `GetResourcePolicy` reports it back, and `DeleteResourcePolicy` clears it. An ARN that names a change set names no entity resource, so the policy operations should refuse it the way they already refuse every other ARN that identifies no resource of this catalog (the modelled `ResourceNotFoundException`, 404) rather than storing a policy on a resource kind the operation is not documented for.

## Evidence

Model (smithy_query query="doc"): PutResourcePolicy — "Attaches a resource-based policy to an entity. Examples of an entity include: AmiProduct and ContainerProduct."; PutResourcePolicyRequest$ResourceArn — "The Amazon Resource Name (ARN) of the entity resource you want to associate with a resource policy."; GetResourcePolicy — "Gets a resource-based policy of an entity that is identified by its resource ARN."; DeleteResourcePolicy — "Deletes a resource-based policy on an entity that is identified by its resource ARN." Contrast: ListTagsForResource — "Lists all tags that have been added to a resource (either an entity or change set)"; TagResourceRequest$ResourceArn — "The Amazon Resource Name (ARN) associated with the resource you want to tag."

The crate's shared parse is justified by the TAGGING doc and then reused by the policy handlers: `services/.../src/state.rs:2710-2721` — "/// `ListTagsForResource` is documented for \"a resource (either an entity or\n/// change set)\", and [`entity_arn`] / [`change_set_arn`] write both in the one\n/// form `<catalog>/<type>/<id>`, so one parse covers both." above `pub enum CatalogResource { Entity { entity_type: String, id: String }, ChangeSet { id: String } }`; `state.rs:2735-2740` — `pub fn policy_key(&self) -> String { match self { Self::Entity { entity_type, id } => format!("{entity_type}/{id}"), Self::ChangeSet { id } => format!("{CHANGE_SET_RESOURCE_TYPE}/{id}") } }`; `state.rs:1541` — `CatalogResource::ChangeSet { id } => self.change_sets.contains_key(id),` inside `holds_resource`.

The handler stores it: `services/.../src/ops/put_resource_policy.rs:157` `let Some(resource) = crate::state::parse_resource_arn(&input.resource_arn) else {`, `:184` `if !store.holds_resource(&resource) {`, `:193-195` `store.resource_policies.insert(resource.policy_key(), input.policy);` — and `get_resource_policy.rs:175` / `delete_resource_policy.rs:171` accept the same change-set resource through `may_carry_policy`, whose own doc quotes the entity-only sentence: `get_resource_policy.rs:171-174` — "// This reports \"a resource-based policy of an entity that is identified\n// by its resource ARN\"".

Existing tests pin the change-set rows (reason stated as "recorded per RESOURCE ARN … for any ARN of this catalog's own form", which the entity-only model sentences contradict): `services/.../tests/all/delete_resource_policy_existence.rs:480-489` — `("the change set that created it, carrying a policy", &ami_change_set_arn, true), ("the change set that created it, carrying none", &saas_change_set_arn, false),`.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"p\"}"}]} and note the returned ChangeSetArn.
2. POST /PutResourcePolicy with {"ResourceArn":"<that ChangeSetArn>","Policy":"{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"aws-marketplace:DescribeEntity\"}]}"}.
3. Observe HTTP 200 and a stored policy, even though the ARN names a change set and the member is documented as "the ARN of the entity resource"; expected is the modelled ResourceNotFoundException (404) that the same handler already answers for an ARN naming no resource of this catalog.
4. GET /GetResourcePolicy?resourceArn=<that ChangeSetArn> — the policy attached to a change set is reported back, confirming it was persisted.
