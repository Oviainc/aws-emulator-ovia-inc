Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: fa5ed22240e89dcfd43a22a078f364ee68f69599

# CreateDistribution stores certificateName with none of the checks its sibling AttachCertificateToDistribution applies, so a distribution can name a certificate that does not exist

_Filed: 2026-10-25T09:20:00Z_
_Operation: CreateDistribution | Class: validation_

> _Note: this issue was identified against the code base at commit `fa5ed22240e89dcfd43a22a078f364ee68f69599`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDistribution` copies the request's `certificateName` straight onto the stored distribution record with no lookup and no state check. The sibling handler in this same crate that performs the identical mutation on the identical field — `AttachCertificateToDistribution`, which sets `distribution.certificate_name` — refuses two things before it writes: a certificate name the account holds no certificate under (Lightsail's `DoesNotExist` envelope) and a certificate that is not `ISSUED` (the modelled `OperationFailureException`). `CreateDistribution` skips both, so a create naming a nonexistent or still-`PENDING_VALIDATION` certificate succeeds with HTTP 200 and leaves a distribution whose `certificateName` points at nothing the account holds. The crate's own `certificate_in_use_resource_count` then counts that distribution against the ghost name, so later creating a certificate under that name makes it report `inUseResourceCount: 1` although nothing ever attached it. (This is NOT a complaint about `alternativeDomainNames` being empty at create: tests/all/distribution_certificate_attachment.rs:812-815 pins publication to the attach with a stated reason.)

## Evidence

CreateDistribution writes the name unchecked — src/ops/create_distribution.rs:382-385:
```
                // The certificate the request named is recorded as asked for.
                // `AttachCertificateToDistribution` /
                // `DetachCertificateFromDistribution` are what change it later.
                certificate_name: input.certificate_name.clone(),
```
Nothing earlier in the handler reads `store.certificate(..)`; the only store lookups are `resolve_origin` (create_distribution.rs:321) and the name/capacity guards (create_distribution.rs:344-355).

The sibling that makes the same attachment applies both checks — src/ops/attach_certificate_to_distribution.rs:204-220:
```
            let certificate = store.certificate(&certificate_name).ok_or_else(|| {
                crate::scaffolded::Scaffolded::resource_does_not_exist(
                    crate::state::RESOURCE_TYPE_CERTIFICATE,
                    &certificate_name,
                )
            })?;
            // A certificate the certificate authority has not issued yet secures
            // nothing, so there is no HTTPS traffic the distribution could accept
            // with it.
            if !certificate.is_issued() {
                return Err(operation_failure(format!(
                    "The certificate {certificate_name} is {}: only certificates with a \
                     status of {} can be attached to a distribution.",
```
and it then writes the very same field, attach_certificate_to_distribution.rs:248:
```
            distribution.certificate_name = Some(certificate_name.clone());
```

The ghost-name consequence is in the crate's own counter — src/state.rs:10529-10535:
```
    pub fn certificate_in_use_resource_count(&self, name: &str) -> i32 {
        let count = self
            .distributions
            .values()
            .filter(|distribution| distribution.certificate_name.as_deref() == Some(name))
            .count();
```

Test coverage: the refusal table that pins both checks covers ONLY the attach — tests/all/distribution_certificate_attachment.rs:713-735 (`REFUSALS`, rows "a certificate name the account does not hold" / "neither resource exists") and the pending-certificate test at line 890 onward. No test exercises CreateDistribution with a certificateName the account does not hold, and tests/all/distribution_certificate_attachment.rs:817-854 only covers create-time naming of an already-ISSUED certificate.

## Steps to Reproduce

1. Against the us-east-1 endpoint, call CreateInstances to create a running instance "origin-1" in us-east-1a (the distribution origin).
2. Call CreateDistribution with distributionName="ghost-cdn", bundleId="small_1_0", origin={"name":"origin-1"}, defaultCacheBehavior={"behavior":"cache"} and certificateName="no-such-cert" — no certificate of that name has ever been created.
3. Observe HTTP 200 and distribution.certificateName == "no-such-cert", where AttachCertificateToDistribution with the same name answers HTTP 400 NotFoundException / code DoesNotExist.
4. Call GetDistributions — the stored record still reports certificateName "no-such-cert".
5. Now call CreateCertificate with certificateName="no-such-cert" and read GetCertificates with includeCertificateDetails=true: certificateDetail.inUseResourceCount is 1 although no attach was ever made.
6. Variant: CreateCertificate "pending-cert", then immediately CreateDistribution with certificateName="pending-cert" — accepted with 200, while AttachCertificateToDistribution refuses it with OperationFailureException ("only certificates with a status of ISSUED can be attached to a distribution").
