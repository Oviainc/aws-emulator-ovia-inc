Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9bf726e3fb20c68357b67dbe525252e1e62354e

# The origin endpoint's configuration fingerprint digests the attached policy, although its own comment, the trait contract and the Channel copy beside it all exclude a sub-resource

_Filed: 2026-10-29T18:21:15Z_
_Operation: UpdateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9bf726e3fb20c68357b67dbe525252e1e62354e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`VersionedRecord::configuration_fingerprint` is the crate's one answer to "did this write CHANGE the record?", and its contract is explicit: everything the record IS, minus the commit bookkeeping and minus "the sub-resources hanging off it". `Channel`'s implementation honours that by clearing `own.policy`, `own.origin_endpoints` and `own.ingested_objects` before digesting. `OriginEndpoint`'s implementation clears only `harvest_jobs` — and its comment says it does so "for the same reason the attached policy is" left out, a reason the code below it does not act on: `own.policy` is never cleared, so an endpoint's "content" digest includes the whole `EndpointPolicy` (document plus CDN auth block) that `PutOriginEndpointPolicy` stores and `DeleteOriginEndpointPolicy` removes.

The endpoint's policy is a sub-resource by this crate's own definition — the field's doc says so and says it "takes no part in the endpoint's ETag" — and both policy writers deliberately commit nothing, so the one rule that decides whether an endpoint commits a new version is computed over a value the crate says is not part of the endpoint. Today nothing mutates the policy and the configuration in a single call, so no response differs; the defect is that the no-op-write rule for origin endpoints is the only one of the four that is not the rule it documents, and nothing in the crate or its tests holds it to that. The fix is one line (`own.policy = None;`), bringing the endpoint impl in line with the sibling channel impl, the trait contract, and the field's own doc.

## Evidence

The contract the four implementations share — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:893-899:
```
    /// A fingerprint of everything this record IS, except the bookkeeping a commit writes
    /// (its commit count, its entity tag and its modification instant) and the
    /// sub-resources hanging off it, which commit versions of their own.
    ///
    /// Compared only within one request: derived `Debug` output is not a stability
    /// contract, so the value is never stored or returned.
    fn configuration_fingerprint(&self) -> String;
```

The sibling copy, which DOES scrub the attached policy — src/state.rs:952-967:
```
impl VersionedRecord for Channel {
    fn configuration_fingerprint(&self) -> String {
        ...
        // The attached policy, the origin endpoints and what the ingest data plane
        // accepted are not channel members — no `Channel*` shape in the model carries
        // them and they take no part in the channel's entity tag either — so they are no
        // part of its content here.
        own.policy = None;
        own.origin_endpoints = BTreeMap::new();
        own.ingested_objects = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }
```

The endpoint copy, whose comment claims the policy is left out while the code keeps it — src/state.rs:974-985:
```
impl VersionedRecord for OriginEndpoint {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The harvest jobs are left out for the same reason the attached policy is: a
        // sub-resource commits versions of its own.
        own.harvest_jobs = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }
```
(no `own.policy = None;` anywhere in the body)

The field the digest therefore covers, with the rule it is declared under — src/state.rs:1251-1259:
```
    /// The resource policy attached to this origin endpoint, or `None` while none is.
    ///
    /// The policy is a sub-resource of the endpoint rather than a resource of its own —
    /// `Put*` replaces it wholesale, `Get*` reports it and `Delete*` detaches it — ...
    /// gates the endpoint's egress data plane (see `crate::service`), and being a
    /// sub-resource it takes no part in the endpoint's ETag.
    pub policy: Option<EndpointPolicy>,
```

The callers of the endpoint fingerprint — src/ops/update_origin_endpoint.rs:515 (`let before = endpoint.configuration_fingerprint();`) and :578 (`endpoint.commit_if_changed(&before, crate::state::now());`), plus src/state.rs:747-749 in `with_tags_mut`.

The writer that stores the sub-resource now inside that digest, committing nothing — src/ops/put_origin_endpoint_policy.rs:256-259:
```
        endpoint.policy = Some(crate::state::EndpointPolicy {
            policy: input.policy,
            cdn_auth_configuration: input.cdn_auth_configuration,
        });
```

Test coverage: the no-op-write rule for endpoints is pinned by tests/all/update_origin_endpoint.rs:436 `an_update_that_stores_the_same_content_reports_the_same_etag`, and `grep -i policy tests/all/update_origin_endpoint.rs` returns no match — no test ever attaches a policy before an update, so neither spelling of the fingerprint is held to the documented rule.

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs and read the `VersionedRecord::configuration_fingerprint` doc at lines 893-899 ("... and the sub-resources hanging off it").
2. Read `impl VersionedRecord for Channel` at lines 952-967: `own.policy = None;` is applied, with the reason stated in the comment above it.
3. Read `impl VersionedRecord for OriginEndpoint` at lines 974-985: the comment says the harvest jobs are left out "for the same reason the attached policy is", but the body never clears `own.policy`, so the attached `EndpointPolicy` is digested as endpoint content.
4. Read the field's own doc at lines 1251-1259, which states the policy is a sub-resource that "takes no part in the endpoint's ETag".
5. Note that the only writers of `endpoint.policy` (src/ops/put_origin_endpoint_policy.rs:256 and src/ops/delete_origin_endpoint_policy.rs:178) deliberately commit nothing, so the endpoint's commit decision is being computed over a value no operation is allowed to make it depend on.
