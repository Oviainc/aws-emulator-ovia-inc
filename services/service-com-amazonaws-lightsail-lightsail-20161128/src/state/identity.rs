// SPDX-License-Identifier: CC0-1.0

//! How this service names itself and its resources: the ARN namespace, the ARN
//! resource prefixes and the `ResourceType` values each stored resource reports,
//! and the defaults a request that names neither a region nor a key pair is
//! served with.

/// This service's ARN namespace (also its sigv4 signing name and endpoint
/// prefix), from `smithy_query(query="service_info")`.
pub const ARN_SERVICE: &str = "lightsail";

/// The resource portion an instance ARN carries before its id, from the modelled
/// `Instance$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:Instance/244ad76f-…`).
pub const INSTANCE_ARN_RESOURCE_PREFIX: &str = "Instance/";

/// The `resourceType` an instance and its operations report, from the modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_INSTANCE: &str = "Instance";

/// The `resourceType` a container service reports, from the same modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_CONTAINER_SERVICE: &str = "ContainerService";

/// The resource portion a container service ARN carries before its id. Mirrors
/// the `Instance/<uuid>` form the modelled `Instance$arn` documentation shows,
/// with this resource's own `ResourceType` value.
pub const CONTAINER_SERVICE_ARN_RESOURCE_PREFIX: &str = "ContainerService/";

/// The region a request that carries no SigV4 credential scope is served in.
///
/// Lightsail's own default endpoint region, and one of the modelled `RegionName`
/// enum's values, so a resource created by an unsigned call still reports a
/// region the model admits.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `resourceType` a bucket and its operations report, from the same modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_BUCKET: &str = "Bucket";

/// The `resourceType` a managed relational database and its operations report,
/// from the same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_RELATIONAL_DATABASE: &str = "RelationalDatabase";

/// The resource portion a managed relational database's ARN carries before its
/// id. Mirrors the `Instance/<uuid>` form the modelled `Instance$arn`
/// documentation shows, with this resource's own `ResourceType` value.
pub const RELATIONAL_DATABASE_ARN_RESOURCE_PREFIX: &str = "RelationalDatabase/";

/// The `resourceType` a managed relational database SNAPSHOT reports, from the
/// same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_RELATIONAL_DATABASE_SNAPSHOT: &str = "RelationalDatabaseSnapshot";

/// The resource portion a managed relational database snapshot's ARN carries
/// before its id. Mirrors the `Instance/<uuid>` form the modelled `Instance$arn`
/// documentation shows, with this resource's own `ResourceType` value.
pub const RELATIONAL_DATABASE_SNAPSHOT_ARN_RESOURCE_PREFIX: &str = "RelationalDatabaseSnapshot/";

/// The resource portion a bucket ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const BUCKET_ARN_RESOURCE_PREFIX: &str = "Bucket/";

/// The resource portion a domain ARN carries before its id, from the modelled
/// `Domain$arn` documentation
/// (`arn:aws:lightsail:global:123456789101:Domain/824cede0-…`).
pub const DOMAIN_ARN_RESOURCE_PREFIX: &str = "Domain/";

/// The `resourceType` a domain and its operations report, from the modelled
/// `ResourceType` enum.
pub const RESOURCE_TYPE_DOMAIN: &str = "Domain";

/// The ARN region token of a domain recordset: a DNS zone is not a regional
/// resource, which is why the modelled `Domain$arn` documentation shows `global`
/// where an instance ARN carries a region.
pub const DOMAIN_ARN_REGION: &str = "global";

/// The region a domain recordset reports in its `ResourceLocation`.
///
/// `ResourceLocation$regionName` targets the modelled `RegionName` enum, which
/// has no `global` member, and the modelled `InvalidInputException`
/// documentation says "Domain and distribution APIs are only available in the
/// N. Virginia (us-east-1) Amazon Web Services Region" — so that is the region a
/// recordset of this account lives in.
pub const DOMAIN_REGION_NAME: &str = "us-east-1";

/// The refusal a domain or distribution call made outside [`DOMAIN_REGION_NAME`]
/// is answered with, as the modelled `InvalidInputException` documentation
/// states it (`smithy_query(query="doc",shape="InvalidInputException")`): those
/// APIs are served by the N. Virginia endpoint ALONE, so a call to them from any
/// other Region's endpoint is refused rather than served there.
pub const DOMAIN_REGION_ONLY_MESSAGE: &str = "Domain and distribution APIs are only available in \
     the N. Virginia (us-east-1) Amazon Web Services Region. Please set your Amazon Web Services \
     Region configuration to us-east-1 to create, view, or edit these resources.";

/// The resource portion a disk ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const DISK_ARN_RESOURCE_PREFIX: &str = "Disk/";

/// The `resourceType` a block storage disk and its operations report, from the
/// modelled `ResourceType` enum — the value the modelled `Disk$resourceType`
/// documentation names ("The Lightsail resource type (`Disk`)").
pub const RESOURCE_TYPE_DISK: &str = "Disk";

/// The resource portion a disk snapshot ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const DISK_SNAPSHOT_ARN_RESOURCE_PREFIX: &str = "DiskSnapshot/";

/// The `resourceType` a disk snapshot and its operations report, from the
/// modelled `ResourceType` enum — the value the modelled
/// `DiskSnapshot$resourceType` documentation names ("The Lightsail resource type
/// (`DiskSnapshot`)").
pub const RESOURCE_TYPE_DISK_SNAPSHOT: &str = "DiskSnapshot";

/// The resource portion an instance snapshot ARN carries before its id, from the
/// modelled `InstanceSnapshot$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:InstanceSnapshot/d23b5706-…`).
pub const INSTANCE_SNAPSHOT_ARN_RESOURCE_PREFIX: &str = "InstanceSnapshot/";

/// The `resourceType` an instance snapshot and its operations report, from the
/// modelled `ResourceType` enum — the value the modelled
/// `InstanceSnapshot$resourceType` documentation names ("The type of resource
/// (usually `InstanceSnapshot`)").
pub const RESOURCE_TYPE_INSTANCE_SNAPSHOT: &str = "InstanceSnapshot";

/// The resource portion an export snapshot record ARN carries before its id.
/// Mirrors the `Instance/<uuid>` form the modelled `Instance$arn` documentation
/// shows, with this resource's own `ResourceType` value.
pub const EXPORT_SNAPSHOT_RECORD_ARN_RESOURCE_PREFIX: &str = "ExportSnapshotRecord/";

/// The `resourceType` an export snapshot record reports, from the modelled
/// `ResourceType` enum — the value the modelled
/// `ExportSnapshotRecord$resourceType` documentation names ("The Lightsail
/// resource type (`ExportSnapshotRecord`)").
pub const RESOURCE_TYPE_EXPORT_SNAPSHOT_RECORD: &str = "ExportSnapshotRecord";

/// The resource portion a static IP ARN carries before its id, from the modelled
/// `StaticIp$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:StaticIp/9cbb4a9e-…`).
pub const STATIC_IP_ARN_RESOURCE_PREFIX: &str = "StaticIp/";

/// The `resourceType` a static IP and its operations report, from the modelled
/// `StaticIp$resourceType` documentation ("the resource type (usually
/// `StaticIp`)") and the same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_STATIC_IP: &str = "StaticIp";

/// The `resourceType` an SSH key pair and its operations report, from the
/// modelled `KeyPair$resourceType` documentation ("The resource type (usually
/// `KeyPair`)") and the same modelled `ResourceType` enum.
pub const RESOURCE_TYPE_KEY_PAIR: &str = "KeyPair";

/// The resource portion a key pair ARN carries before its id, from the modelled
/// `KeyPair$arn` documentation
/// (`arn:aws:lightsail:us-east-2:123456789101:KeyPair/05859e3d-…`).
pub const KEY_PAIR_ARN_RESOURCE_PREFIX: &str = "KeyPair/";

/// The `resourceType` an alarm and its operations report, from the same modelled
/// `ResourceType` enum — the value the modelled `Alarm$resourceType` member
/// carries ("The Lightsail resource type of the alarm").
pub const RESOURCE_TYPE_ALARM: &str = "Alarm";

/// The `resourceType` a contact method and its operations report, from the same
/// modelled `ResourceType` enum — the value the modelled
/// `ContactMethod$resourceType` member carries ("The Lightsail resource type of
/// the contact method").
pub const RESOURCE_TYPE_CONTACT_METHOD: &str = "ContactMethod";

/// The `resourceType` the peering of a Region's Lightsail VPC with the account's
/// default VPC reports, from the same modelled `ResourceType` enum — its
/// `PeeredVpc` member, the only one that names a VPC peering.
pub const RESOURCE_TYPE_PEERED_VPC: &str = "PeeredVpc";

/// The `resourceType` a load balancer and its operations report, from the same
/// modelled `ResourceType` enum — the value the modelled
/// `LoadBalancer$resourceType` member carries ("The resource type
/// (`LoadBalancer`").
///
/// It is also what keys the alarm metric catalog, because the modelled
/// `PutAlarmRequest$metricName` documentation names load balancers as one of the
/// three kinds of resource an alarm monitors.
pub const RESOURCE_TYPE_LOAD_BALANCER: &str = "LoadBalancer";

/// The `resourceType` an SSL/TLS certificate of a load balancer and its
/// operations report, from the same modelled `ResourceType` enum — the value the
/// modelled `LoadBalancerTlsCertificate$resourceType` member carries ("The
/// resource type (`LoadBalancerTlsCertificate`)").
pub const RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE: &str = "LoadBalancerTlsCertificate";

/// The resource portion a load balancer ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const LOAD_BALANCER_ARN_RESOURCE_PREFIX: &str = "LoadBalancer/";

/// The resource portion an alarm ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const ALARM_ARN_RESOURCE_PREFIX: &str = "Alarm/";

/// The resource portion a contact method ARN carries before its id. Mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows, with
/// this resource's own `ResourceType` value.
pub const CONTACT_METHOD_ARN_RESOURCE_PREFIX: &str = "ContactMethod/";

/// Applied when `CreateInstances` omits `ipAddressType`.
///
/// From that member's own documentation: "The default value is `dualstack`."
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "dualstack";

/// The key pair an instance reports when the request named none: the Lightsail
/// default key pair, which the modelled `Instance$sshKeyName` documentation
/// names as its example (`LightsailDefaultKeyPair`).
pub const DEFAULT_SSH_KEY_NAME: &str = "LightsailDefaultKeyPair";
