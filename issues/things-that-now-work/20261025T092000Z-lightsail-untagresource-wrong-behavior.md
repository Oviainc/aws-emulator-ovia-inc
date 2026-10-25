Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# UntagResource cannot reach a certificate's tags, so a certificate created with tags can never be untagged

_Filed: 2026-10-25T09:20:00Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` resolves the named resource through the crate's single tag-resolution helper, `ComAmazonawsLightsailLightsail20161128State::with_tagged_record`. That helper's own doc-comment states the rule it implements: it admits "the ones whose modelled shape carries a `tags` member". The modelled `Certificate` carries a `tags` member, `CreateCertificate` stores the request's tags verbatim on `StoredCertificate.tags`, and `GetCertificates` reports them back — but `with_tagged_record` never looks in `store.certificates`, and `StoredCertificate` is absent from the `tagged_records!` list that implements the `TaggedRecord` trait. Consequently an `UntagResource` call naming a certificate of the account falls through every arm, `with_tagged_record` answers `None`, and the handler answers Lightsail's `DoesNotExist` refusal ("resourceName '<name>' does not name a resource of this account in <region>") for a certificate the same emulator will happily list with its tags. The tags written at create are therefore permanently unremovable.

## Evidence

The rule the helper states, services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:11195-11197:
```
    /// The one place a tag change resolves a resource, so every operation that
    /// edits tags admits exactly the same set of resources — the ones whose
    /// modelled shape carries a `tags` member.
```
The set it actually admits, state.rs:11204-11259 — `instances`, `disks`, `disk_snapshots`, `instance_snapshots`, `domains`, `relational_databases`, `relational_database_snapshots`, `alarms`, `distributions`, `buckets`, `container_services`, `load_balancers`, `contact_methods` — and nothing else, ending at state.rs:11259 with `None`. The trait list confirms the omission, state.rs:11407-11421:
```
tagged_records!(
    StoredInstance,
    StoredDisk,
    StoredDiskSnapshot,
    StoredInstanceSnapshot,
    StoredDomain,
    StoredRelationalDatabase,
    StoredRelationalDatabaseSnapshot,
    StoredAlarm,
    StoredContactMethod,
    StoredDistribution,
    StoredBucket,
    StoredContainerService,
    StoredLoadBalancer,
);
```
(no `StoredCertificate`).

The certificate DOES carry tags. state.rs:9041 on `StoredCertificate`:
```
    pub tags: Vec<Tag>,
```
and they are stored from the request, src/ops/create_certificate.rs:220 and :265:
```
        let tags = input.tags.unwrap_or_default();
...
                tags,
```
The model agrees — smithy_query(query="shape", shape="com.amazonaws.lightsail#Certificate") lists member `"tags": {"target": "TagList", "type": "list", "doc": "<p>The tag keys and optional values for the resource..."}`.

The refusal path, src/ops/untag_resource.rs:192 and :212-214:
```
            let resolved = store.with_tagged_record(&region, &name, |record, resource_type| {
...
            let Some(resolved) = resolved else {
                return Err(not_found(&name, &region));
            };
```
Test coverage: tests/all/resource_tags.rs exercises tag removal, and tests/all/distribution_certificate_attachment.rs creates certificates (`create_certificate`) — neither covers untagging a certificate.

## Steps to Reproduce

1. Call CreateCertificate with certificateName="my-cert", domainName="example.com" and tags=[{"key":"env","value":"prod"}] against the us-east-1 endpoint.
2. Call GetCertificates with certificateName="my-cert" — the answer carries tags=[{"key":"env","value":"prod"}], proving the tag was stored on the certificate.
3. Call UntagResource with resourceName="my-cert" and tagKeys=["env"].
4. Observe HTTP 400 with wire error NotFoundException / body code DoesNotExist and the message "resourceName 'my-cert' does not name a resource of this account in us-east-1", instead of the tag being removed.
5. Call GetCertificates again — the tag is still there, and no operation of the crate can remove it.
