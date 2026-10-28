Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# A load balancer's SSL/TLS certificate cannot be tagged or untagged, on the strength of a state comment that says its shape carries no tags member

_Filed: 2026-10-27T14:59:33Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate resolves every tag change through one helper, `with_tagged_record`, whose own doc states the rule it implements: it admits "the ones whose modelled shape carries a `tags` member". The helper walks instances, disks, both snapshot kinds, domains, databases, database snapshots, alarms, key pairs, distributions, certificates, buckets, container services, load balancers and contact methods — and never the SSL/TLS certificates a load balancer holds. The companion `active_names` doc justifies that omission by asserting the load balancer TLS certificate is one of "the kinds whose modelled shape carries no `tags` member" — which the model contradicts: `LoadBalancerTlsCertificate` carries a `tags` member, `CreateLoadBalancerTlsCertificate` bounds the request's `tags` by `MAX_TAGS` and stores them verbatim on the record, and `active_names` itself reports those certificates' names as active names of the account.

So a `TagResource` (and `UntagResource`) call naming a load balancer certificate that `GetActiveNames` has just reported falls through every arm of `with_tagged_record`, which answers `None`, and the handler refuses it with Lightsail's `DoesNotExist` envelope ("resourceName '<name>' does not name a resource of this account in <region>"). The tags written at create are therefore permanently unreachable — the exact defect already fixed for the `Certificate` kind (issue 20261025T092000Z-lightsail-untagresource-wrong-behavior), with the load balancer's certificate left behind.

## Evidence

The rule the shared helper states — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:14479-14481:
```
    /// The one place a tag change resolves a resource, so every operation that
    /// edits tags admits exactly the same set of resources — the ones whose
    /// modelled shape carries a `tags` member.
```
The premise the omission rests on — src/state.rs:14405-14410:
```
    /// are addressable there. Beside them are the kinds whose modelled shape
    /// carries no `tags` member, so no tag operation resolves them — the static
    /// IP, the export snapshot record, the CloudFormation stack record, and the
    /// SSL/TLS certificate a load balancer holds.
```
The model says otherwise — smithy_query(query="shape", shape="LoadBalancerTlsCertificate") lists `"tags": {"target": "TagList", "type": "list", "doc": "<p>The tag keys and optional values for the resource."}` (while `StaticIp`, `ExportSnapshotRecord` and `CloudFormationStackRecord` genuinely carry none, so the sentence is wrong about this one kind only).

The tags ARE accepted and stored — src/ops/create_load_balancer_tls_certificate.rs:251-258 and :310-318:
```
        let tags = input.tags.unwrap_or_default();
        if tags.len() > crate::state::MAX_TAGS {
...
            balancer
                .certificates
                .push(crate::state::StoredLoadBalancerCertificate {
                    name: certificate_name.clone(),
                    domain_name,
                    alternative_names,
                    is_attached: false,
                    tags,
                });
```
The same certificate IS an active name of the account — src/state.rs:14439-14444:
```
        names.extend(
            self.load_balancers_in(region)
                .into_iter()
                .flat_map(|balancer| balancer.certificates.iter())
                .map(|certificate| certificate.name.clone()),
        );
```
The arms that resolve a tag change, src/state.rs:14488-14554, end at `None` with no `balancer.certificates` lookup, and the refusal it produces is src/ops/tag_resource.rs:213 + :246-248:
```
            let resolved = store.with_tagged_record(&region, &name, |record, resource_type| {
...
            let Some(resolved) = resolved else {
                return Err(not_found(&name, &region));
            };
```
(`UntagResource` takes the identical path at src/ops/untag_resource.rs:192 and :212-214.)

Test coverage that states the rule and omits this kind — tests/all/tagged_resource_coverage.rs:14-22:
```
//! The rule under test: a tag change reaches EVERY resource of the account whose
//! modelled shape carries a `tags` member, each resolved by the name its own
//! reader serves it under — so a tag written at create is removable, and the
//! removal is observable through that reader. `resource_tags.rs` pins the same
//! rule for the instance, the block storage disk, the managed database, the
//! bucket, the load balancer and the contact method; the kinds below are the
//! rest of the class. The DNS zone is the one tag-carrying kind no case here can
//! drive: ...
```
No case in that suite, nor in tests/all/resource_tags.rs, nor in tests/all/load_balancer_tls_certificate_create.rs, tags a load balancer certificate, so the omission is unpinned.

## Steps to Reproduce

1. Call CreateLoadBalancer with loadBalancerName="lb-1", instancePort=80 against us-east-1.
2. Call CreateLoadBalancerTlsCertificate with loadBalancerName="lb-1", certificateName="lb-cert", certificateDomainName="example.com" and tags=[{"key":"env","value":"prod"}] — it succeeds and the tags are stored on StoredLoadBalancerCertificate (src/ops/create_load_balancer_tls_certificate.rs:317).
3. Call GetActiveNames — "lb-cert" is reported among the account's active names (src/state.rs:14439-14444).
4. Call TagResource with resourceName="lb-cert" and tags=[{"key":"team","value":"cdn"}] (or UntagResource with tagKeys=["env"]).
5. Observe HTTP 400, wire error NotFoundException, body code DoesNotExist, message "resourceName 'lb-cert' does not name a resource of this account in us-east-1" — although the model's LoadBalancerTlsCertificate carries a tags member and the crate stores one.
