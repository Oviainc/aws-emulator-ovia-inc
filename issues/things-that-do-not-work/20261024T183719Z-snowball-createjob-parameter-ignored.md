Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# CreateJob for a cluster node silently drops every job-level member the request supplied (Description, Notification, RoleARN, Resources, ShippingOption, …)

_Filed: 2026-10-24T18:37:19Z_
_Operation: CreateJob | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `CreateJob` is called with a `ClusterId`, the handler builds the new job purely from the cluster record (`cluster.node_job(..)`) and then copies back only four members of the request: `LongTermPricingId`, `DeviceConfiguration`, `ImpactLevel` and `PickupDetails`. Every other member the caller sent is discarded without any error: `Description`, `Notification`, `Resources`, `RoleARN`, `KmsKeyARN`, `ShippingOption`, `SnowballCapacityPreference`, `TaxDocuments`, `RemoteManagement`, `AddressId` and `ForwardingAddressId`.

Expected: the model says a node request *need not* repeat the cluster's attributes ("you need to provide only this clusterId value"), not that supplied values are thrown away. `CreateJobRequest$Description` is documented as "an optional description of this specific job" — per job, not per cluster — so a node created with `{ClusterId, Description: "node 1"}` is stored and described with that description. The same holds for the per-job notification settings a caller asks for.

Actual: `DescribeJob` on the new node reports the cluster's description (or none) instead of the one submitted, and reports the cluster's notification/resources/role, so a caller cannot tell its five node jobs apart and silently loses configuration it asked for with a 200 response. As a second symptom of the same dropped-input path, a supplied `AddressId` / `ForwardingAddressId` is not even validated in this branch (the non-cluster branch and `CreateCluster` both answer `InvalidResourceException` for an unknown address id), so `{ClusterId: <valid>, AddressId: "ADID0000…"}` naming no stored address answers 200.

## Evidence

Spec/model: `smithy_query(query="doc", shape="CreateJobRequest", member="Description")` — "Defines an optional description of this specific job, for example Important Photos 2016-08-11"; `CreateJobRequest$ClusterId` — "If you're creating a job for a node in a cluster, you need to provide only this clusterId value. The other job attributes are inherited from the cluster" ("only need to provide" = the remaining members stay accepted inputs); `InvalidResourceException` — "The specified resource can't be found. Check the information you provided in your last request, and try again."

Handler: services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_job.rs, the `if let Some(cluster_id) = input.cluster_id` branch (lines ~314-401): after `cluster.node_job(job_id, now, unlock_code)` only `node.long_term_pricing_id`, `node.device_configuration`, `node.impact_level` and `node.pickup_details` are assigned from `input` (lines 390-393); `input.description`, `input.notification`, `input.resources`, `input.role_arn`, `input.kms_key_arn`, `input.shipping_option`, `input.snowball_capacity_preference`, `input.tax_documents`, `input.remote_management`, `input.address_id` and `input.forwarding_address_id` are never read in this branch, and the address existence check at lines 418-444 is only reached when no `ClusterId` was given. `StoredCluster::node_job` (src/state.rs ~line 697) fills those members from the cluster.

Tests: tests/all/cluster_updates.rs `create_job_joins_a_cluster_until_it_holds_its_quorum` (line ~586) and `a_node_job_takes_the_clusters_job_and_device_type_or_is_refused` (line ~716) only submit `ClusterId` (plus a mismatching JobType/SnowballType), so no test submits a per-node `Description`/`Notification` and nothing asserts what happens to it.

## Steps to Reproduce

1. CreateAddress with a deliverable address -> AddressId A.
2. CreateCluster with {JobType: LOCAL_USE, SnowballType: EDGE, ShippingOption: STANDARD, AddressId: A, Description: "cluster", ForceCreateJobs: false} -> ClusterId.
3. CreateJob with {ClusterId, Description: "node 1", Notification: {SnsTopicARN: "arn:aws:sns:us-east-1:123456789012:snow", NotifyAll: true}, ShippingOption: EXPRESS} -> 200 with a JobId.
4. DescribeJob on that JobId -> JobMetadata.Description is "cluster" (the cluster's) instead of "node 1", Notification is the cluster's (absent), ShippingDetails.ShippingOption is STANDARD instead of EXPRESS.
5. CreateJob with {ClusterId, AddressId: "ADID00000000-0000-0000-0000-000000000000"} -> 200 instead of the InvalidResourceException the same unknown address id produces when no ClusterId is supplied.
