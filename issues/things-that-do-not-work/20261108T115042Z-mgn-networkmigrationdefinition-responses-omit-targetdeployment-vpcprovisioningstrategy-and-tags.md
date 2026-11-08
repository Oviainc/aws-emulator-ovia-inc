Issue-Source: trace-sweep-divergence
Issue-Trace: mgn-network-migration-definition-partial-update-preserves-other-fields
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mgn-network-migration-definition-partial-update-preserves-other-fields
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|UpdateNetworkMigrationDefinition|2xx->2xx|$.tags
Issue-Key: mgn|UpdateNetworkMigrationDefinition|2xx->2xx|$.targetDeployment
Issue-Key: mgn|UpdateNetworkMigrationDefinition|2xx->2xx|$.vpcProvisioningStrategy
Issue-Key: mgn|GetNetworkMigrationDefinition|2xx->2xx|$.tags
Issue-Key: mgn|GetNetworkMigrationDefinition|2xx->2xx|$.targetDeployment
Issue-Key: mgn|GetNetworkMigrationDefinition|2xx->2xx|$.vpcProvisioningStrategy
Issue-Commit: cfd5bf7f29cd7ef3bd739b81b9df1c10c1a91447

# mgn NetworkMigrationDefinition responses omit targetDeployment, vpcProvisioningStrategy and tags

> _Note: this issue was identified against the code base at commit `cfd5bf7f29cd7ef3bd739b81b9df1c10c1a91447`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateNetworkMigrationDefinition

## Description
Two causes, one shared fault: the mgn NetworkMigrationDefinition representation the emulator serves drops three modeled members — `targetDeployment`, `vpcProvisioningStrategy` and `tags` — so neither the update response nor the subsequent read response carries the values the definition was created with. Both operations return the same output shape (`NetworkMigrationDefinition`), so the omission is in how that resource is stored and/or serialized, not in one handler's argument parsing.

Cause 1 of 2: UpdateNetworkMigrationDefinition (fixture mgn-network-migration-definition-partial-update-preserves-other-fields, step task#4).
AWS: Smithy (com.amazonaws.mgn) — UpdateNetworkMigrationDefinition is POST /network-migration/UpdateNetworkMigrationDefinition, http code 200, output shape NetworkMigrationDefinition, whose members include `targetDeployment` (TargetDeployment), `vpcProvisioningStrategy` (VpcProvisioningStrategy) and `tags` (TagsMap, @length min 0 max 50, @sensitive). Every member of the input is optional except `networkMigrationDefinitionID`, i.e. an update names only the members it changes and AWS returns the whole, merged definition: members the request did not mention keep the value they were created/last updated with, and a `tags` map that is present but empty is serialized as `{}` (an empty map is a value, not an absent member). So an update that does not mention `targetDeployment`/`vpcProvisioningStrategy` must still echo the stored values, and the `tags` member must appear.
Emulator: replay of the partial update returns 200 but the body omits all three members — `$.targetDeployment` missing (expected 'SINGLE_ACCOUNT' as stored by the create step, got None), `$.vpcProvisioningStrategy` missing (expected 'CREATE_NEW', got None), `$.tags` missing (expected {}, got None).
Why: a client that partially updates a definition cannot tell what the definition now is. Fields it did not touch read back as absent, which an SDK surfaces as null/unset — so callers conclude the deployment mode and VPC provisioning strategy were cleared by their update, and tooling that re-submits the read-back object (a read-modify-write or a CDK/Terraform-style refresh) will wipe or re-default those settings. An absent `tags` member likewise is indistinguishable from "tags unsupported" rather than "no tags".
Fix: the UpdateNetworkMigrationDefinition handler's state merge plus the NetworkMigrationDefinition serializer in the mgn service crate — persist `targetDeployment`, `vpcProvisioningStrategy` and `tags` on the stored definition, merge an update over the stored record instead of rebuilding it from the request, and emit the `tags` member even when the map is empty.
Traces: mgn-network-migration-definition-partial-update-preserves-other-fields#task#4.

Cause 2 of 2: GetNetworkMigrationDefinition (same fixture, step task#5).
AWS: Smithy — GetNetworkMigrationDefinition is POST /network-migration/GetNetworkMigrationDefinition, http code 200, @readonly, output shape NetworkMigrationDefinition (same three members as above). A read of an existing definition returns the full stored representation, including `targetDeployment`, `vpcProvisioningStrategy` and `tags`.
Emulator: the read after the partial update returns 200 but the body again omits all three — `$.targetDeployment` missing (expected 'SINGLE_ACCOUNT'), `$.vpcProvisioningStrategy` missing (expected 'CREATE_NEW'), `$.tags` missing (expected {}).
Why: the authoritative read confirms the loss rather than correcting it, so the fields are unrecoverable through the API: a client has no way to discover the deployment target or VPC provisioning strategy of a definition it created, and tag-aware callers see no tag container at all.
Fix: the GetNetworkMigrationDefinition handler / the shared NetworkMigrationDefinition projection and the definition record in the mgn service's state — the same stored fields and serializer path as cause 1; both operations project the one resource shape.
Traces: mgn-network-migration-definition-partial-update-preserves-other-fields#task#5.

No open mgn issue covers this: the existing open mgn issues concern InitializeService's 204-instead-of-ValidationException, 501 NotImplemented operations (tag API, CreateLaunchConfigurationTemplate, CreateWave, CreateConnector, StartNetworkMigrationAnalysis, DeleteNetworkMigrationDefinition, CreateReplicationConfigurationTemplate stubs) and the missing Content-Length on empty-body 204 tag responses — none of them is about dropped NetworkMigrationDefinition response members.

## Evidence
Replay of fixture mgn-network-migration-definition-partial-update-preserves-other-fields (trace set simparity-wire-aws-v1):

Step task#4 — UpdateNetworkMigrationDefinition, expected status 200, actual 200:
  $.targetDeployment: missing (expected 'SINGLE_ACCOUNT', got None)
  $.vpcProvisioningStrategy: missing (expected 'CREATE_NEW', got None)
  $.tags: missing (expected {}, got None)

Step task#5 — GetNetworkMigrationDefinition, expected status 200, actual 200:
  $.targetDeployment: missing (expected 'SINGLE_ACCOUNT', got None)
  $.vpcProvisioningStrategy: missing (expected 'CREATE_NEW', got None)
  $.tags: missing (expected {}, got None)

Model evidence (smithy_query, com-amazonaws-mgn-applicationmigrationservice): UpdateNetworkMigrationDefinition and GetNetworkMigrationDefinition both have output_shape NetworkMigrationDefinition with members targetDeployment: TargetDeployment, vpcProvisioningStrategy: VpcProvisioningStrategy, tags: TagsMap (@length 0..50, @sensitive); Update's only required input member is networkMigrationDefinitionID, so all other members are optional on a partial update. Statuses matched in both steps; only the body members diverged. The expected enum-ish values above are this recording's inputs filling the template — the requirement is that whatever the definition was created/updated with is echoed back, and that an empty tag map serializes as {}.

## Steps to Reproduce
1. POST /network-migration/CreateNetworkMigrationDefinition with name, targetS3Configuration, targetNetwork, plus targetDeployment and vpcProvisioningStrategy set (the fixture uses SINGLE_ACCOUNT / CREATE_NEW) and no tags.
2. POST /network-migration/UpdateNetworkMigrationDefinition with only networkMigrationDefinitionID and one unrelated member (e.g. description) — a partial update.
3. Observe the 200 body: targetDeployment, vpcProvisioningStrategy and tags are absent instead of carrying the created values and an empty {} tag map.
4. POST /network-migration/GetNetworkMigrationDefinition with the same networkMigrationDefinitionID and observe the same three members absent from the 200 body.
Equivalently: replay fixture mgn-network-migration-definition-partial-update-preserves-other-fields, steps task#4 and task#5.
