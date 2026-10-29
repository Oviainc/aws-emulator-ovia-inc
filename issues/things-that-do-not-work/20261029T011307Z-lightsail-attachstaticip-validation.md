Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# AttachStaticIp pairs a static IP with an instance in ANOTHER Region, the location check every other cross-resource pairing in this crate applies

_Filed: 2026-10-29T01:13:07Z_
_Operation: AttachStaticIp | Class: validation_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AttachStaticIp` resolves both halves of the attachment by NAME alone and never compares where the two records are. The crate holds that a static IP is a Region's resource — `AllocateStaticIp` records it with `StoredLocation::regional(&region)` on the strength of "A static IP belongs to one Region" — and the instance is held by name alone with its own `location`, so the two can be in different Regions and this handler will still attach them.

Every other operation in this crate that pairs two resources refuses exactly that: `AttachDisk` refuses a disk and an instance in different Availability Zones, `AttachInstancesToLoadBalancer` refuses an instance and a load balancer in different Regions, and `SetResourceAccessForBucket` filters the named instance to the Region the bucket is in before granting it access. `AttachStaticIp` enumerates its own extra preconditions in a comment — "both resources have to be ones this account holds, neither may already hold another attachment, and the instance has to have the kind of address a static IP is" — and location is not among them.

The resulting state is self-contradictory and observable: `state::attach_static_ip` writes the reserved address onto the instance (`instance.public_ip_address = Some(ip_address)`, `instance.is_static_ip = true`), so `GetInstance` reports an instance in, say, `us-east-1a` serving on a public IPv4 address this account reserved in `eu-west-1`, while `GetStaticIp` reports that eu-west-1 reservation `attachedTo` an instance whose own `location.regionName` is `us-east-1`. `StartInstance`'s `assign_addresses_for_start` then deliberately KEEPS that foreign address across a stop/start, so the cross-Region pairing persists for the life of the instance.

Note on fix placement: `tests/all/static_ip_attach.rs:661` allocates `spare-ip` in `eu-west-1` and uses it against instances in `us-east-1a` in three refusal rows that expect the already-attached / no-IPv4 refusals by their own messages, so a location refusal must be evaluated AFTER those existing preconditions to leave those rows' responses unchanged.

## Evidence

The crate's own statement that a static IP belongs to one Region — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/allocate_static_ip.rs:160-169:
```
        // A static IP belongs to one Region: Lightsail's API is regional, and
        // the modelled `StaticIp$location` is "the region and Availability Zone
        // where the static IP was created" — so the Region of the request's
        // SigV4 credential scope is what the allocation is recorded in.
        let region = crate::state::request_region(ctx.parts());
...
        let location = crate::state::StoredLocation::regional(&region);
```
(the modelled member agrees — smithy_query(query="doc", shape="StaticIp", member="location"): "The region and Availability Zone where the static IP was created.")

The handler's own enumeration of the checks it makes, which contains no location rule — src/ops/attach_static_ip.rs:168-178:
```
        // `staticIpName` and `instanceName` are modelled `@required` members
        // carrying the modelled `ResourceName` pattern, so the centralized walk
        // refused an absent or malformed one before this body ran. What is left is
        // what the model cannot express: both resources have to be ones this
        // account holds, neither may already hold another attachment, and the
        // instance has to have the kind of address a static IP is. Every one of
        // those is decided BEFORE the first write, so a refused request attaches
        // nothing.
        let static_ip_name = input.static_ip_name;
        let instance_name = input.instance_name;
```
and the resolution itself, which reads the static IP's `location` only to place the log entry and never compares it with the instance's — src/ops/attach_static_ip.rs:187-196:
```
            let static_ip = store.static_ip(&static_ip_name).ok_or_else(|| {
                not_found(crate::state::RESOURCE_TYPE_STATIC_IP, &static_ip_name)
            })?;
            let location = static_ip.location.clone();
            let attached_to = static_ip.attached_to.clone();

            let instance = store.instance(&instance_name).ok_or_else(|| {
                not_found(crate::state::RESOURCE_TYPE_INSTANCE, &instance_name)
            })?;
            let instance_ip_address_type = instance.ip_address_type.clone();
```
(the last precondition it does apply is the IPv4 one at src/ops/attach_static_ip.rs:238-244; grep for `region` in that file returns nothing.)

The three siblings that DO refuse a cross-location pairing:
* src/ops/attach_disk.rs:265-281:
```
            // A disk is block storage of one Availability Zone, and an instance
            // runs in one: the modelled `CreateDisk$availabilityZone`
            // documentation says to "use the same Availability Zone as the
            // Lightsail instance to which you want to attach the disk", so a
            // disk in another zone cannot reach this instance.
            if location.availability_zone != instance_zone {
                return Err(operation_failure(format!(
                    "The disk {disk_name} is in {} and the instance {instance_name} is in \
                     {}: a disk can be attached only to an instance in the same \
                     Availability Zone.",
```
* src/ops/attach_instances_to_load_balancer.rs:249-262:
```
                if instance.location.region_name != balancer_location.region_name {
                    return Err(operation_failure(format!(
                        "The instance {name} is in {} and the load balancer \
                         {load_balancer_name} is in {}: an instance can be attached only to \
                         a load balancer in the same Amazon Web Services Region.",
```
* src/ops/set_resource_access_for_bucket.rs:214-224:
```
            // "The name of the Lightsail instance for which to set bucket
            // access": the resource is an INSTANCE, so a name that does not
            // address one of this Region's instances addresses nothing this call
            // can grant access to. An instance is held by name alone, so its own
            // location is what decides whether it is in this Region.
            let instance = store
                .instance(&input.resource_name)
                .filter(|instance| instance.location.region_name == region)
```

What the unchecked attach writes — src/state.rs:13411-13419:
```
    pub fn attach_static_ip(&mut self, name: &str, instance_name: &str) -> Option<String> {
        let ip_address = self.static_ips.get(name)?.ip_address.clone();
...
        let instance = self.instance_mut(instance_name)?;
        instance.is_static_ip = true;
        instance.public_ip_address = Some(ip_address.clone());
        self.static_ips.get_mut(name)?.attached_to = Some(instance_name.to_string());
```
and the state module's own note that the preconditions are the handler's to make — src/state.rs:13406-13410:
```
    /// Attaching the static IP an instance already holds writes exactly what is
    /// already stored, so the call is idempotent; whether an attachment may be
    /// made at all — the static IP is free, the instance holds none, the instance
    /// has public IPv4 addressing — is the calling operation's question, decided
    /// before this is reached.
```

Test coverage: tests/all/static_ip_attach.rs:147-200 keeps every success row's static IP Region and instance `availabilityZone` in the SAME Region (e.g. `region: Some("us-west-2")` with `availability_zone: "us-west-2a"`), and the refusal suite's stated rule (tests/all/static_ip_attach.rs:642-647) lists only unknown names, an existing attachment on either side, and an instance with no public IPv4 address. No test attaches a reservation to an instance in another Region and asserts success, so no existing behaviour is pinned.

## Steps to Reproduce

1. Call AllocateStaticIp with staticIpName="eu-reservation", signed for eu-west-1 — answered 200, and GetStaticIp reports location.regionName "eu-west-1".
2. Call CreateInstances with instanceNames=["ohio-host"], availabilityZone="us-east-2a", a blueprintId and bundleId, and wait for the instance to settle into "running" — its location.regionName is "us-east-2".
3. Call AttachStaticIp with staticIpName="eu-reservation" and instanceName="ohio-host".
4. Observe HTTP 200: GetInstance("ohio-host") now reports publicIpAddress = the address reserved in eu-west-1 and isStaticIp = true, while GetStaticIp("eu-reservation") reports attachedTo "ohio-host" and isAttached true — one record says eu-west-1, the other us-east-2.
5. Call StopInstance then StartInstance for "ohio-host": src/ops/start_instance.rs:233-249 keeps the attached static IP's address, so the instance is re-provisioned onto the foreign Region's address again.
6. Compare src/ops/attach_static_ip.rs:168-196 with src/ops/attach_disk.rs:265-281, src/ops/attach_instances_to_load_balancer.rs:249-262 and src/ops/set_resource_access_for_bucket.rs:214-224: the three siblings refuse the cross-location pairing, this one never looks at either record's location.
