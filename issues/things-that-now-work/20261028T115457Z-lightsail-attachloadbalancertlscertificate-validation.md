Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# AttachLoadBalancerTlsCertificate attaches a certificate that is still PENDING_VALIDATION, the rule its own state comment states and its distribution sibling enforces

_Filed: 2026-10-28T11:54:57Z_
_Operation: AttachLoadBalancerTlsCertificate | Class: validation_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A load-balancer SSL/TLS certificate is stored `PENDING_VALIDATION` by `CreateLoadBalancerTlsCertificate` (and by `CreateLoadBalancer`), with the clock owing it `ISSUED`. `AttachLoadBalancerTlsCertificate` then attaches whichever certificate the request names with a single check — does the load balancer hold a certificate of that name — and never asks whether the certificate authority has issued it. The store's own field comment states the opposite rule ("A certificate is attached only by `AttachLoadBalancerTlsCertificate`, never by the call that requested it: it has to be validated first"), the record carries a ready-made predicate for it (`StoredLoadBalancerCertificate::is_issued`, used only to render views), and the crate's other certificate kind has a single owner of exactly this rule (`issued_certificate`, "the ONE owner of that rule") which `AttachCertificateToDistribution` and `CreateDistribution` both ask FIRST.

So a caller that creates a certificate and attaches it in the next call gets HTTP 200, and the load balancer immediately reports the `HTTP_HTTPS` protocol and port 443 terminating HTTPS with a certificate the service has not validated and whose `status` the same response still reports as `PENDING_VALIDATION` — a self-contradictory record. The attach is also the operation the model describes as following validation.

## Evidence

The rule, written on the stored record — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:10862-10866:
```
    /// Whether the certificate is attached to the load balancer, which is what
    /// `LoadBalancerTlsCertificateSummary$isAttached` reports. A certificate is
    /// attached only by `AttachLoadBalancerTlsCertificate`, never by the call
    /// that requested it: it has to be validated first.
    pub is_attached: bool,
```
The crate's predicate for "validated", which the attach never calls — src/state.rs:10885-10889:
```
impl StoredLoadBalancerCertificate {
    /// Whether the certificate authority has issued this certificate.
    pub fn is_issued(&self) -> bool {
        self.status == LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED
    }
```
What the attach actually checks — src/ops/attach_load_balancer_tls_certificate.rs:228-233:
```
            if !balancer.attach_certificate(&certificate_name) {
                return Err(not_found(
                    crate::state::RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE,
                    &certificate_name,
                ));
            }
```
and the helper it calls, which only resolves the NAME — src/state.rs:11245-11253:
```
    pub fn attach_certificate(&mut self, name: &str) -> bool {
        if self.certificate_index(name).is_none() {
            return false;
        }
        for certificate in &mut self.certificates {
            certificate.is_attached = certificate.name == name;
        }
        true
    }
```
The certificate really is pending when the create returns — src/ops/create_load_balancer_tls_certificate.rs:317-346:
```
            // The certificate is not valid yet: Lightsail validates its domains
            // before the certificate authority issues it, so the record is
            // stored `PENDING_VALIDATION` and the clock owes it `ISSUED`, which
            // is why a read straight after this call still reports the pending
            // status and no issuance date.
...
                status: crate::state::LOAD_BALANCER_TLS_CERTIFICATE_STATUS_PENDING_VALIDATION,
                due: Some(due),
```
(and the same file's :326-331 "Lightsail validates it first and `AttachLoadBalancerTlsCertificate` is what attaches it").

The sibling that DOES enforce it — src/ops/attach_certificate_to_distribution.rs:204-211:
```
            // A certificate this account does not hold, and one the certificate
            // authority has not issued yet (which secures nothing, so there is no
            // HTTPS traffic the distribution could accept with it), are both
            // refused by the store's one owner of that rule
            let certificate = store
                .issued_certificate(&certificate_name)
                .map_err(|refusal| certificate_refusal(&certificate_name, refusal))?;
```
backed by src/state.rs:14062-14087 ("the ONE owner of that rule ... a request naming a certificate this account does not hold, or one still being validated, is refused before anything is written").

Model text (smithy_query doc, AttachLoadBalancerTlsCertificate): "Once you create and validate your certificate, you can attach it to your load balancer."

Test coverage: tests/all/load_balancer_tls_certificate_attach.rs attaches immediately after the create for the rows whose `settle_first` is false (:270-280 "the only certificate, no alternative domains, attached while provisioning", :291-308), but its stated variation is "whether the load balancer has settled into `active` before the attach" (:28) — no case states a reason for attaching an unissued certificate, and no case asserts the certificate's `status` at attach time, so the behaviour is not pinned on purpose.

## Steps to Reproduce

1. POST /ls/api/2016-11-28/CreateLoadBalancer (or CreateLoadBalancerTlsCertificate) for load balancer "lb" with certificateName "lb-cert" and certificateDomainName "example.com".
2. Immediately (before the clock owes the certificate `ISSUED`, src/ops/create_load_balancer_tls_certificate.rs:322-325) call AttachLoadBalancerTlsCertificate with loadBalancerName="lb", certificateName="lb-cert".
3. Observe HTTP 200: the attach is served and GetLoadBalancer reports tlsCertificateSummaries[0].isAttached=true, protocol HTTP_HTTPS and publicPorts [80, 443] while GetLoadBalancerTlsCertificates still reports that certificate's status as PENDING_VALIDATION.
4. Compare with the same sequence for a distribution — CreateCertificate then AttachCertificateToDistribution before the clock issues it — which src/ops/attach_certificate_to_distribution.rs:209-211 refuses through `issued_certificate`.
