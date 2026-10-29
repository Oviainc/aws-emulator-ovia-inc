Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 355761efac79959eac560a8efce6608069de81f3

# TagResource edits a domain, distribution or certificate from any Region, skipping the us-east-1 restriction every other door onto those records applies

_Filed: 2026-10-28T21:24:40Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `355761efac79959eac560a8efce6608069de81f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly one owner of the "Domain and distribution APIs are only available in us-east-1" rule, `crate::state::domain_region_refusal`, and 21 handlers call it — every domain operation (CreateDomain, DeleteDomain, GetDomain, GetDomains, CreateDomainEntry, UpdateDomainEntry, DeleteDomainEntry), every distribution operation (Create/Update/Delete/GetDistributions, GetDistributionBundles, GetDistributionMetricData, GetDistributionLatestCacheReset, ResetDistributionCache, UpdateDistributionBundle, Attach/DetachCertificateToDistribution) and every certificate operation (CreateCertificate, DeleteCertificate, GetCertificates), plus SetIpAddressType when the request names a distribution.

TagResource (and UntagResource, which shares the resolution) is also a door onto those same three kinds of record — `with_tagged_record` resolves `self.domains`, `self.distributions` and `self.certificates` by NAME ALONE, so the request's Region never narrows them — and neither handler asks the helper. A TagResource request whose SigV4 credential scope names, say, eu-west-1 therefore succeeds in writing tags onto a domain, a distribution or a certificate: the handler reads the Region (`request_region`) only to resolve the Region-keyed kinds (bucket, container service, load balancer) and runs no restriction check at all.

The result is two doors onto one record that disagree about whether the Region serves it: in eu-west-1 the tag write is answered 200 and mutates the record, while GetDomain/GetDomains/GetCertificates/GetDistributions for the very same record in the same Region answer the modelled InvalidInputException — so the tag just written cannot be read back through the Region that accepted the write. The model states the rule for exactly the verb TagResource performs ("to create, view, or edit these resources"), and TagResource declares the InvalidInputException that carries it.

Fix scope: a tag write resolved to a domain, distribution or certificate record should go through the crate's one owner of the rule, as the handler's own sibling doors on those records do; the Region-keyed kinds (bucket, container service, load balancer) are unaffected.

## Evidence

The crate's one owner of the rule, which states that every operation it covers must call it — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:16294-16301:
```
/// The same documented restriction covers the distribution (and distribution
/// certificate) operations: `CreateDistribution` and `GetDistributions` call
/// THIS helper, and so must any other one of them when it is implemented,
/// rather than re-deriving the rule — so the restriction stays in one place.
pub fn domain_region_refusal(parts: &::http::request::Parts) -> Option<String> {
    let signed = signed_region(parts)?;
    (signed != DOMAIN_REGION_NAME).then(|| DOMAIN_REGION_ONLY_MESSAGE.to_string())
}
```

The modelled sentence it carries, from the error TagResource itself declares — smithy_query(query="doc", shape="InvalidInputException"), quoted verbatim in state.rs:176-181 and in tests/all/domain_region_restriction.rs:39-41:
```
Domain and distribution APIs are only available in the N. Virginia (us-east-1) Amazon Web
Services Region. Please set your Amazon Web Services Region configuration to us-east-1 to
create, view, or edit these resources.
```
(TagResource's modelled errors include `InvalidInputException` (HTTP 400) — smithy_query(query="operation", operation="TagResource").)

A sibling door states that the restriction binds the READ as much as the write — src/ops/get_certificates.rs:174-185:
```
        // ALONE: the modelled `InvalidInputException` documentation covers
        // "Domain and distribution APIs" together and says to set the Region to
        // us-east-1 "to create, VIEW, or edit these resources", so a read is as
        // restricted as a create. Checked through the crate's one owner of that
        // rule, before anything is read.
        if let Some(message) = crate::state::domain_region_refusal(ctx.parts()) {
            return Err(invalid(message));
        }

        // Past that guard a certificate is not addressed per-Region (see
        // `crate::state::DISTRIBUTION_REGION_NAME`), so one name names one record
        // of the account.
```
and the store says the same about the record itself — src/state.rs:12984-12990:
```
    /// Every SSL/TLS certificate, keyed by its certificate name — the identifier
    /// every certificate operation takes. NOT keyed by region, for the same
    /// reason a distribution is not: a Lightsail certificate is addressed from
    /// the one Region its API is served in (see [`DISTRIBUTION_REGION_NAME`]).
```

The tagging door skips it — src/ops/tag_resource.rs:166-177 and :212-213 (no `domain_region_refusal` anywhere in the file; grep for it across src/ops returns 22 files, and neither tag_resource.rs nor untag_resource.rs is among them):
```
        // A tag change is a regional call: a bucket, a container service and a
        // load balancer are identified by the Region they were created in
        // together with their name, so the Region of the request's SigV4
        // credential scope is what `resourceName` is resolved in — the same
        // resolution `UntagResource` makes, through the same store helper.
        let region = crate::state::request_region(ctx.parts());
...
            let resolved = store.with_tagged_record(&region, &name, |record, resource_type| {
```
and that shared resolution reaches the three restricted kinds by name alone, before the Region is used at all — src/state.rs:15304-15329:
```
        if let Some(record) = self.domains.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DOMAIN));
        }
...
        if let Some(record) = self.distributions.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_DISTRIBUTION));
        }
        // A certificate is held under its name alone, the name
        // `GetCertificates` serves it by, so a tag change resolves it that way
        // too — exactly like the distribution it is attached to.
        if let Some(record) = self.certificates.get_mut(name) {
            return Some(body(record, RESOURCE_TYPE_CERTIFICATE));
        }
        // Only the Region-keyed maps need the pair, so it is built here rather
        // than above
        let regional = (region.to_string(), name.to_string());
```
(`request_region` falls back to us-east-1 only when the request carries no scope — src/state.rs:16265-16271 — so a scope naming another Region really does reach these arms.)

Test coverage: tests/all/domain_region_restriction.rs:111-118 declares its table "Every implemented domain operation" and lists the seven domain-named operations only — no tagging row, and no stated reason for leaving one out. The tagging suites exercise exactly these kinds but only in the domain Region: tests/all/tagged_resource_coverage.rs:66-78 tags a Certificate and a Distribution, tests/all/resource_tags.rs:50 fixes `DEFAULT_REGION = "us-east-1"`. So no existing test pins a tag write from another Region, and adding the guard leaves every existing test's responses unchanged.

## Steps to Reproduce

1. CreateDomain with domainName "example.com", signed for us-east-1 — answered 200.
2. TagResource with resourceName "example.com" and tags [{"key":"env","value":"prod"}], signed for eu-west-1 (Authorization credential scope .../eu-west-1/lightsail/aws4_request) — answered 200, and the zone record now carries the tag.
3. GetDomain with domainName "example.com", signed for eu-west-1 — refused with InvalidInputException "Domain and distribution APIs are only available in the N. Virginia (us-east-1) Amazon Web Services Region...": the Region that accepted the edit refuses to show the record it edited.
4. GetDomain with domainName "example.com", signed for us-east-1 — reports tags [{"key":"env","value":"prod"}], proving the out-of-Region write landed.
5. The same sequence with CreateCertificate/"coverage-certificate" (or CreateDistribution) + TagResource signed for eu-west-1 behaves identically, while CreateCertificate, GetCertificates and DeleteCertificate signed for eu-west-1 are all refused.
6. UntagResource with resourceName "example.com" and tagKeys ["env"], signed for eu-west-1 — also answered 200, removing the tag through the same unguarded resolution.
