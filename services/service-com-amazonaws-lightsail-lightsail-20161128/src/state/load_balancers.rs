// SPDX-License-Identifier: CC0-1.0

//! Load balancers: the instances registered with one and their health, the
//! attributes it serves under, and the TLS certificates attached to it.

use super::*;

/// `LoadBalancerState` values this slice uses, from the modelled enum and the
/// `LoadBalancer$state` documentation ("The status of your load balancer"): a
/// load balancer Lightsail is still standing up is `provisioning`, and one that
/// is serving is `active`.
///
/// The enum's `active_impaired`, `failed` and `unknown` members describe a load
/// balancer whose instances are unhealthy, one whose creation failed, and one
/// whose status Lightsail cannot report — none of which this store produces,
/// because nothing here fails a provision.
pub const LOAD_BALANCER_STATE_PROVISIONING: &str = "provisioning";
/// See [`LOAD_BALANCER_STATE_PROVISIONING`].
pub const LOAD_BALANCER_STATE_ACTIVE: &str = "active";

/// `LoadBalancerProtocol` values, from the modelled enum and the
/// `LoadBalancer$protocol` documentation: "You can't just have `HTTP_HTTPS`, but
/// you can have just `HTTP`" — so a load balancer with no SSL/TLS certificate
/// ATTACHED serves `HTTP`, and one with an attached certificate serves both.
pub const LOAD_BALANCER_PROTOCOL_HTTP: &str = "HTTP";
/// See [`LOAD_BALANCER_PROTOCOL_HTTP`].
pub const LOAD_BALANCER_PROTOCOL_HTTP_HTTPS: &str = "HTTP_HTTPS";

/// The public ports a load balancer listens on, from the modelled
/// `LoadBalancer$publicPorts` documentation: "For HTTP, use port 80. For HTTPS,
/// use port 443."
pub const LOAD_BALANCER_HTTP_PORT: i32 = 80;
/// See [`LOAD_BALANCER_HTTP_PORT`].
pub const LOAD_BALANCER_HTTPS_PORT: i32 = 443;

/// The health check path a load balancer uses when the request named none, from
/// the modelled `CreateLoadBalancerRequest$healthCheckPath` documentation: "If
/// you didn't specify a health check path, Lightsail uses the root path of your
/// website (`"/"`)."
pub const DEFAULT_HEALTH_CHECK_PATH: &str = "/";

/// The most load balancers this account may hold in ONE Region.
///
/// An AWS quota, not an emulator guard: the modelled `CreateLoadBalancer`
/// documentation states it — "You can create up to 10 load balancers per AWS
/// Region in your account."
pub const MAX_LOAD_BALANCERS_PER_REGION: usize = 10;

/// How many load balancers one `GetLoadBalancers` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more load balancers remain. Kept
/// below [`MAX_LOAD_BALANCERS_PER_REGION`] so an account that holds the most
/// load balancers one Region admits still pages.
pub const LOAD_BALANCER_PAGE_SIZE: usize = 5;

/// The modelled `LoadBalancerAttributeName` members, which are the keys of
/// `LoadBalancer$configurationOptions` ("A string to string map of the
/// configuration options for your load balancer").
pub const LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH: &str = "HealthCheckPath";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_ENABLED: &str = "SessionStickinessEnabled";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_COOKIE_DURATION: &str =
    "SessionStickiness_LB_CookieDurationSeconds";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_HTTPS_REDIRECTION_ENABLED: &str = "HttpsRedirectionEnabled";
/// See [`LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH`].
pub const LOAD_BALANCER_ATTRIBUTE_TLS_POLICY_NAME: &str = "TlsPolicyName";

/// The session-stickiness cookie lifetime a load balancer starts with, in
/// seconds.
///
/// A Lightsail load balancer IS an Elastic Load Balancer, whose
/// `stickiness.lb_cookie.duration_seconds` attribute AWS documents as
/// "`86400` seconds (1 day)" by default; Lightsail surfaces that same attribute
/// as `SessionStickiness_LB_CookieDurationSeconds`.
pub const DEFAULT_SESSION_STICKINESS_COOKIE_DURATION_SECONDS: i64 = 86_400;

/// The shortest session-stickiness cookie lifetime that attribute admits, in
/// seconds.
///
/// Exactly the source [`DEFAULT_SESSION_STICKINESS_COOKIE_DURATION_SECONDS`]
/// comes from: the Elastic Load Balancing load balancer attribute
/// `stickiness.lb_cookie.duration_seconds`, which Lightsail surfaces as
/// `SessionStickiness_LB_CookieDurationSeconds`, is documented as "The range is
/// 1 second to 1 week (604800 seconds)". Lightsail's own model states only that
/// the value "must be an integer that represents the cookie duration in
/// seconds", so the attribute's published range is what bounds it.
pub const MIN_SESSION_STICKINESS_COOKIE_DURATION_SECONDS: i64 = 1;

/// The longest session-stickiness cookie lifetime that attribute admits, in
/// seconds — the one week of the same published range. See
/// [`MIN_SESSION_STICKINESS_COOKIE_DURATION_SECONDS`].
pub const MAX_SESSION_STICKINESS_COOKIE_DURATION_SECONDS: i64 = 604_800;

/// The health check path `path` describes, or `None` when it describes none.
///
/// `LoadBalancer$healthCheckPath` is a `NonEmptyString` (pattern `\S`), so a
/// path that carries nothing but whitespace is not one any read of this resource
/// could ever answer; and the member is "the path to ping on the target", which
/// the whitespace around it is no part of, so what is stored is the path
/// itself. The one owner of that rule, shared by every operation that takes a
/// health check path from a request.
pub fn health_check_path_value(path: &str) -> Option<String> {
    let trimmed = path.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether this service publishes a TLS security policy named `name`.
///
/// Both `CreateLoadBalancer$tlsPolicyName` and the `TlsPolicyName` attribute of
/// `UpdateLoadBalancerAttribute` document the same rule — "Use the
/// GetLoadBalancerTlsPolicies action to get a list of TLS policy names that you
/// can specify" — so a name that catalog does not report names no policy a load
/// balancer can be given.
pub fn publishes_load_balancer_tls_policy(name: &str) -> bool {
    load_balancer_tls_policy_spec(name).is_some()
}

/// The refusal message for a TLS security policy name this service does not
/// publish, naming the request member that carried it and the names that are
/// available instead.
pub fn unpublished_tls_policy_message(member: &str, name: &str) -> String {
    format!(
        "{member} '{name}' is not a TLS security policy this service publishes: \
         GetLoadBalancerTlsPolicies reports {}",
        reported_load_balancer_tls_policy_names(&load_balancer_tls_policies())
    )
}

/// The DNS suffix a load balancer's `dnsName` is published under.
///
/// AWS publishes it under its own `elb.amazonaws.com` zone. That HOSTNAME is
/// environment-specific and no caller may assume it, while the PROMISE
/// `LoadBalancer$dnsName` makes is not — it is "The DNS name of your Lightsail
/// load balancer", so it must never be a name that sends the next client OUT of
/// the emulator. This emulator owns no DNS zone, so the name keeps the ELB SHAPE
/// (`<load balancer name>-<id>.<region>`) under a suffix that belongs to the
/// emulator and cannot resolve off-machine, exactly as a managed database's
/// master endpoint does ([`RELATIONAL_DATABASE_ENDPOINT_SUFFIX`]): `.local` is
/// reserved for link-local multicast name resolution (RFC 6762 §3).
///
/// This emulator terminates no HTTP traffic on it: a load balancer's DATA plane
/// is not served. Everything the control plane owns — the record, its state,
/// its ports, its configuration options and its certificate summaries — is
/// real.
pub(super) const LOAD_BALANCER_DNS_SUFFIX: &str = "lb.lightsail.local";

/// How many characters of the load balancer's own id go into its `dnsName`: the
/// 10-character suffix ELB puts between the load balancer's name and its Region
/// (`my-load-balancer-1234567890.us-east-2.elb.amazonaws.com`).
pub(super) const LOAD_BALANCER_DNS_ID_LEN: usize = 10;

/// The ARN of the load balancer `id` in `region`.
pub fn load_balancer_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LOAD_BALANCER_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `dnsName` of the load balancer `name` in `region`, minted as `id`. See
/// [`LOAD_BALANCER_DNS_SUFFIX`].
pub fn load_balancer_dns_name(region: &str, name: &str, id: &str) -> String {
    let token: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(LOAD_BALANCER_DNS_ID_LEN)
        .collect();
    format!("{name}-{token}.{region}.{LOAD_BALANCER_DNS_SUFFIX}")
}

/// The most SSL/TLS certificates ONE load balancer may hold at a time.
///
/// An AWS quota, not an emulator guard: the modelled
/// `CreateLoadBalancerTlsCertificate$certificateName` documentation states it —
/// "Each Lightsail load balancer can have up to 2 certificates associated with
/// it at one time."
pub const MAX_CERTIFICATES_PER_LOAD_BALANCER: usize = 2;

/// The most SSL/TLS certificates of load balancers this account may hold at a
/// time, across every Region.
///
/// An AWS quota too, from the same documentation: "You can have up to 10
/// certificates in your account at one time."
pub const MAX_LOAD_BALANCER_TLS_CERTIFICATES: usize = 10;

/// The most alternative domains and subdomains ONE load balancer certificate
/// may also cover.
///
/// An AWS quota too, from the modelled
/// `CreateLoadBalancerTlsCertificate$certificateAlternativeNames`
/// documentation: "You can have a maximum of 9 alternative names (in addition
/// to the 1 primary domain)."
pub const MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES: usize = 9;

/// The most ENTRIES an alternative-name list may carry before it is de-duped.
///
/// Emulator resource guard, NOT an AWS quota, and the bound that makes the
/// de-duping affordable: the names a request repeats are only known to be
/// repeats once every entry has been examined, so the number of entries a
/// request may send is capped BEFORE any of that work runs rather than after
/// it. Generous next to the
/// [`MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES`] distinct domains
/// that can survive de-duping; a list carrying more entries than this is
/// refused with the modelled `InvalidInputException`.
pub const MAX_DOMAIN_NAME_LIST_ENTRIES: usize = 100;

/// The alternative domains a certificate actually covers, given the primary
/// domain it is for and the names a request listed.
///
/// "Lightsail will de-dupe the names for you": a name the list repeats, and a
/// name that already IS the certificate's primary domain, add no domain to the
/// certificate. The ONE owner of that reduction, so every operation that
/// stores a certificate's alternative domains stores the same shape. The walk
/// is bounded by [`MAX_DOMAIN_NAME_LIST_ENTRIES`], which every caller checks
/// before reading the list.
pub fn alternative_domain_names(primary: &str, requested: Vec<String>) -> Vec<String> {
    let mut covered: Vec<String> = Vec::new();
    for name in requested {
        if name != primary && !covered.contains(&name) {
            covered.push(name);
        }
    }
    covered
}

/// The alternative domains a certificate covers, given the primary domain it is
/// for and the list the request member `member` carried, or the refusal message
/// that member earned.
///
/// The ONE owner of the rule ANY requested alternative-name list has to
/// satisfy, whichever certificate the request describes: every entry has to be
/// a domain name (which is also what refuses the wildcards the documentation
/// says are not supported, because `*` is not a character a DNS label carries),
/// and the list the de-duping LEAVES has to be within `max_covered` — the
/// maximum both certificate kinds document "in addition to the primary domain",
/// applied to what [`alternative_domain_names`] leaves rather than to a list
/// whose repeats Lightsail itself would have dropped.
///
/// Whether an entry is a repeat is only known once it has been examined, so how
/// many entries the list may carry is bounded HERE, before a single one is
/// looked at: every validation and every de-dupe comparison below is then
/// bounded by [`MAX_DOMAIN_NAME_LIST_ENTRIES`] rather than by what the caller
/// sent.
pub(super) fn requested_alternative_domain_names(
    member: &str,
    primary: &str,
    requested: Vec<String>,
    max_covered: usize,
) -> Result<Vec<String>, String> {
    if requested.len() > MAX_DOMAIN_NAME_LIST_ENTRIES {
        return Err(format!(
            "{member} carries {} entries, more than the \
             {MAX_DOMAIN_NAME_LIST_ENTRIES} this service reads for one certificate",
            requested.len()
        ));
    }
    for name in &requested {
        validate_domain_name(name).map_err(|detail| {
            format!(
                "{member} entry '{name}' is not an alternative domain of this certificate: \
                 {detail}"
            )
        })?;
    }
    let covered = alternative_domain_names(primary, requested);
    if covered.len() > max_covered {
        return Err(format!(
            "{member} names {} alternative domains, more than the {max_covered} a \
             certificate may have in addition to its primary domain",
            covered.len()
        ));
    }
    Ok(covered)
}

/// The alternative domains a load balancer's SSL/TLS certificate covers for the
/// `certificateDomainName`/`certificateAlternativeNames` pair a request asked
/// for, or the refusal message those members earned.
///
/// The ONE owner of the rule a request describing a certificate of a load
/// balancer has to satisfy, whichever operation carries those members: the
/// primary domain and every alternative name have to be domain names (which is
/// also what refuses the wildcards the documentation says are not supported,
/// because `*` is not a character a DNS label carries), and the de-duped list
/// has to be within the documented
/// [`MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES`].
///
/// "Lightsail will de-dupe the names for you": a name the list repeats, and a
/// name that already IS the primary domain, add no domain to the certificate —
/// so the documented maximum is applied to what [`alternative_domain_names`]
/// leaves rather than to a list whose repeats Lightsail itself would have
/// dropped.
///
/// The entry bound, the per-entry validation and the de-duping are
/// [`requested_alternative_domain_names`]'s, the one owner of that rule for
/// every certificate this store holds; what is this path's own is the member
/// names the refusals have to carry and the maximum this certificate kind
/// documents.
pub fn load_balancer_certificate_alternative_names(
    domain_name: &str,
    requested: Vec<String>,
) -> Result<Vec<String>, String> {
    validate_domain_name(domain_name).map_err(|detail| {
        format!(
            "certificateDomainName '{domain_name}' is not the domain name of a \
             certificate: {detail}"
        )
    })?;
    requested_alternative_domain_names(
        "certificateAlternativeNames",
        domain_name,
        requested,
        MAX_LOAD_BALANCER_TLS_CERTIFICATE_ALTERNATIVE_NAMES,
    )
}

/// The resource portion an ARN of a load balancer's SSL/TLS certificate carries
/// before its id. Mirrors the `Instance/<uuid>` form the modelled
/// `Instance$arn` documentation shows, with this resource's own `ResourceType`
/// value.
pub const LOAD_BALANCER_TLS_CERTIFICATE_ARN_RESOURCE_PREFIX: &str = "LoadBalancerTlsCertificate/";

/// The ARN of the load balancer certificate `id` in `region`. A certificate
/// belongs to the load balancer that holds it, which is a regional resource, so
/// it carries that load balancer's Region.
pub fn load_balancer_tls_certificate_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{LOAD_BALANCER_TLS_CERTIFICATE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `LoadBalancerTlsCertificate$status` a certificate this account just
/// asked for is in, from the modelled `LoadBalancerTlsCertificateStatus` enum:
/// Lightsail validates the certificate's domains before the certificate
/// authority issues it. The same spelling as [`CERTIFICATE_STATUS_PENDING_VALIDATION`],
/// which the certificate slice of this store reports.
pub const LOAD_BALANCER_TLS_CERTIFICATE_STATUS_PENDING_VALIDATION: &str =
    CERTIFICATE_STATUS_PENDING_VALIDATION;

/// The `LoadBalancerTlsCertificate$status` of a validated certificate, from the
/// same enum — the terminal status the clock owes a certificate this account
/// asked for.
pub const LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED: &str = CERTIFICATE_STATUS_ISSUED;

/// `LoadBalancerTlsCertificateRenewalSummary$renewalStatus` of an issued
/// certificate, from the modelled `LoadBalancerTlsCertificateRenewalStatus`
/// enum: "Lightsail is attempting to automatically validate the domain names of
/// the certificate. No further action is required." The renewal summary
/// "contains information about the status of Lightsail's managed renewal for the
/// certificate", and there is a renewal to manage only once the certificate
/// authority has issued one — so this is the only renewal status a certificate
/// of this store reports, and a certificate still being validated for the first
/// time reports no renewal summary at all.
pub const LOAD_BALANCER_TLS_CERTIFICATE_RENEWAL_STATUS_PENDING_AUTO_RENEWAL: &str =
    "PENDING_AUTO_RENEWAL";

/// The algorithm the certificate authority signed the certificate with, as
/// `LoadBalancerTlsCertificate$signatureAlgorithm` reports it.
///
/// It follows from [`CERTIFICATE_KEY_ALGORITHM`]: the certificate carries an RSA
/// key, and AWS Certificate Manager — which is what issues a Lightsail SSL/TLS
/// certificate — signs an RSA certificate with SHA-256, so the X.509 signature
/// algorithm is `sha256WithRSAEncryption`, printed in the form a certificate's
/// signature algorithm is reported in.
pub const LOAD_BALANCER_TLS_CERTIFICATE_SIGNATURE_ALGORITHM: &str = "SHA256WITHRSA";

/// The type of the DNS record a domain of a certificate is validated through, as
/// `LoadBalancerTlsCertificateDomainValidationRecord$type` describes it ("The
/// type of validation record. For example, `CNAME` for domain validation").
pub const LOAD_BALANCER_TLS_CERTIFICATE_VALIDATION_RECORD_TYPE: &str = "CNAME";

/// The `subject` of a certificate issued for `domain_name`: "the name of the
/// entity that is associated with the public key contained in the certificate",
/// which for an SSL/TLS certificate is the X.509 subject distinguished name,
/// whose common name is the domain the certificate is for.
pub(super) fn load_balancer_tls_certificate_subject(domain_name: &str) -> String {
    format!("CN={domain_name}")
}

/// The SSL/TLS certificate a load balancer holds: the one
/// `CreateLoadBalancer`'s certificate members asked for, or one
/// `CreateLoadBalancerTlsCertificate` added.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancerCertificate {
    /// The certificate name the request named, kept verbatim — it is what every
    /// certificate operation addresses this certificate by.
    pub name: String,
    /// The ARN this certificate was minted with, from its own id — see
    /// [`load_balancer_tls_certificate_arn`].
    pub arn: String,
    /// The support code built from that same id, so support can look the
    /// certificate up from the value alone ([`support_code`]).
    pub support_code: String,
    /// The instant the request that asked for this certificate was served.
    pub created_at_ms: i64,
    /// Where the certificate is: the location of the load balancer that holds
    /// it, which is the Region that load balancer — and so this certificate —
    /// belongs to.
    ///
    /// Kept on the record, exactly as a certificate of the account keeps one
    /// ([`StoredCertificate::location`]): it is what the modelled `location`
    /// member reports and what locates the operation-log entry a change to this
    /// certificate writes. A load balancer's Region is half its store key, so
    /// this can never drift from the holder's own location.
    pub location: StoredLocation,
    /// The domain the certificate is associated with.
    pub domain_name: String,
    /// The alternative domains and subdomains the certificate also covers, as
    /// [`alternative_domain_names`] reduced them — every writer of this field
    /// goes through that one owner, so a repeat and the primary domain are
    /// never among them.
    pub alternative_names: Vec<String>,
    /// Whether the certificate is attached to the load balancer, which is what
    /// `LoadBalancerTlsCertificateSummary$isAttached` reports. A certificate is
    /// attached only by `AttachLoadBalancerTlsCertificate`, never by the call
    /// that requested it: it has to be validated first.
    pub is_attached: bool,
    /// The tags the request asked for, kept verbatim — "the tag keys and
    /// optional values to add to the resource during create".
    pub tags: Vec<Tag>,
    /// The serial number the certificate was minted with, as
    /// `LoadBalancerTlsCertificate$serial` reports it.
    pub serial: String,
    /// The current `LoadBalancerTlsCertificateStatus`.
    pub status: &'static str,
    /// The status the clock owes this certificate, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instant the certificate authority issues this certificate — the same
    /// instant the clock owes it
    /// [`LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED`] — which is what
    /// `issuedAt`, `notBefore` and `notAfter` are reported from once it is
    /// issued.
    pub issued_at_ms: i64,
}

impl StoredLoadBalancerCertificate {
    /// Whether the certificate authority has issued this certificate.
    pub fn is_issued(&self) -> bool {
        self.status == LOAD_BALANCER_TLS_CERTIFICATE_STATUS_ISSUED
    }

    /// Every domain this certificate covers: the primary domain it is for, then
    /// each alternative domain it also covers.
    ///
    /// The alternative domains went through [`alternative_domain_names`] before
    /// they were stored, so none of them repeats and none of them is the
    /// primary domain — this list is the set of domains the certificate secures
    /// traffic for, with its primary domain first.
    pub fn domain_names(&self) -> Vec<String> {
        let mut names = vec![self.domain_name.clone()];
        names.extend(self.alternative_names.iter().cloned());
        names
    }

    /// The `LoadBalancerTlsCertificateDomainStatus` every domain of this
    /// certificate is in: a certificate the authority has issued is one whose
    /// domains Lightsail validated, and one it has not issued yet is still
    /// waiting for exactly that.
    pub(super) fn domain_validation_status(&self) -> &'static str {
        if self.is_issued() {
            DOMAIN_VALIDATION_STATUS_SUCCESS
        } else {
            DOMAIN_VALIDATION_STATUS_PENDING
        }
    }

    /// This record as the modelled `LoadBalancerTlsCertificate`, on the load
    /// balancer `load_balancer_name` — the load balancer that holds it, which is
    /// what `loadBalancerName` reports and which the certificate is reached
    /// through. `location` comes from this record: it is the location of that
    /// same load balancer, kept here because a change to this certificate has
    /// to locate its own log entry.
    pub fn view(&self, load_balancer_name: &str) -> LoadBalancerTlsCertificate {
        let issued = self.is_issued();
        let validation_status = self.domain_validation_status();
        let domain_names = self.domain_names();
        LoadBalancerTlsCertificate {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_LOAD_BALANCER_TLS_CERTIFICATE.to_string()),
            tags: Some(self.tags.clone()),
            load_balancer_name: Some(load_balancer_name.to_string()),
            is_attached: Some(self.is_attached),
            status: Some(self.status.to_string()),
            domain_name: Some(self.domain_name.clone()),
            // One record per domain the certificate covers, each carrying the
            // domain it is about, the kind of DNS record that validates it and
            // the state that validation is in. `name` is that domain because
            // that is what the member documents it to be: "A fully qualified
            // domain name in the certificate. For example, `example.com`."
            // `value` is left out and so is `dnsRecordCreationState`: this
            // emulator owns no DNS zone to publish a validation record in, so
            // there is no value a caller could create and no creation of one to
            // report — the same choice the certificate slice of this store
            // makes ([`StoredCertificate::detail`]).
            domain_validation_records: Some(
                domain_names
                    .iter()
                    .map(
                        |domain_name| LoadBalancerTlsCertificateDomainValidationRecord {
                            name: Some(domain_name.clone()),
                            r#type: Some(
                                LOAD_BALANCER_TLS_CERTIFICATE_VALIDATION_RECORD_TYPE.to_string(),
                            ),
                            domain_name: Some(domain_name.clone()),
                            validation_status: Some(validation_status.to_string()),
                            ..Default::default()
                        },
                    )
                    .collect(),
            ),
            // Nothing here fails a validation, so no certificate of this store
            // carries a `failureReason`; and none is revoked, which is what
            // `revocationReason` and `revokedAt` are "present only when" —
            // their status is `REVOKED`.
            failure_reason: None,
            revocation_reason: None,
            revoked_at: None,
            // Nothing was issued while the certificate's domains were still
            // being validated, so none of the issuance members is reported
            // until it is: a certificate has no issuer, no validity window and
            // no issue date before the certificate authority gives it one.
            issued_at: issued.then(|| timestamp(self.issued_at_ms)),
            issuer: issued.then(|| CERTIFICATE_ISSUER_CA.to_string()),
            not_before: issued.then(|| timestamp(self.issued_at_ms)),
            not_after: issued
                .then(|| timestamp(self.issued_at_ms.saturating_add(CERTIFICATE_VALIDITY_MILLIS))),
            key_algorithm: Some(CERTIFICATE_KEY_ALGORITHM.to_string()),
            signature_algorithm: Some(
                LOAD_BALANCER_TLS_CERTIFICATE_SIGNATURE_ALGORITHM.to_string(),
            ),
            serial: Some(self.serial.clone()),
            subject: Some(load_balancer_tls_certificate_subject(&self.domain_name)),
            // Lightsail manages the renewal of the certificate it ISSUED:
            // nothing renews a certificate the authority has not issued yet, so
            // a certificate still being validated for the first time reports no
            // renewal summary. The renewal is of the same domains, so its
            // options carry one entry per domain, in the same order the
            // validation records are reported in.
            renewal_summary: issued.then(|| LoadBalancerTlsCertificateRenewalSummary {
                renewal_status: Some(
                    LOAD_BALANCER_TLS_CERTIFICATE_RENEWAL_STATUS_PENDING_AUTO_RENEWAL.to_string(),
                ),
                domain_validation_options: Some(
                    domain_names
                        .iter()
                        .map(
                            |domain_name| LoadBalancerTlsCertificateDomainValidationOption {
                                domain_name: Some(domain_name.clone()),
                                validation_status: Some(validation_status.to_string()),
                            },
                        )
                        .collect(),
                ),
            }),
            // The X.509 subject alternative name extension of the certificate,
            // which covers every domain it secures traffic for — its primary
            // domain among them, because that is the domain its `subject`
            // names. Last, because it takes the one list of covered domains
            // every member above reads.
            subject_alternative_names: Some(domain_names),
        }
    }
}

/// One instance registered with a load balancer: the instance it names, and the
/// instant its deregistration is due while one is outstanding.
///
/// A registration is a record of the load balancer, not of the instance: it is
/// what `instanceHealthSummary` reports one entry per.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancerRegistration {
    /// The instance name the registration was made for, kept verbatim — it is
    /// what `InstanceHealthSummary$instanceName` reports.
    pub name: String,
    /// The instant the clock owes this registration its removal, while a
    /// deregistration is outstanding. While it is set the instance is still
    /// registered and still reported, in the modelled `draining` health state.
    pub drains_at_ms: Option<i64>,
}

impl StoredLoadBalancerRegistration {
    /// Whether this registration is being deregistered.
    pub fn is_draining(&self) -> bool {
        self.drains_at_ms.is_some()
    }

    /// This registration as the modelled `InstanceHealthSummary`.
    pub(super) fn view(&self) -> InstanceHealthSummary {
        // A registered instance starts out `initial`, and the modelled
        // `instanceHealthReason` documentation gives the reason for that:
        // "`Lb.RegistrationInProgress` - The target instance is in the process
        // of being registered with the load balancer." This emulator runs no
        // health checks, so no instance leaves it except by being deregistered,
        // which the same enum names: "`Instance.DeregistrationInProgress` - The
        // target instance is in the process of being deregistered".
        let (health, reason) = if self.is_draining() {
            (
                INSTANCE_HEALTH_DRAINING,
                INSTANCE_HEALTH_REASON_DEREGISTRATION_IN_PROGRESS,
            )
        } else {
            (
                INSTANCE_HEALTH_INITIAL,
                INSTANCE_HEALTH_REASON_REGISTRATION_IN_PROGRESS,
            )
        };
        InstanceHealthSummary {
            instance_name: Some(self.name.clone()),
            instance_health: Some(health.to_string()),
            instance_health_reason: Some(reason.to_string()),
        }
    }
}

/// One stored load balancer: the configuration the caller asked for, the members
/// the service assigned, and the lifecycle it is in.
#[derive(Debug, Clone)]
pub struct StoredLoadBalancer {
    /// The load balancer name the request named, kept verbatim — it is this
    /// record's store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    /// Where the load balancer lives. A load balancer carries no availability
    /// zone: the modelled `LoadBalancer$location` documentation says "Lightsail
    /// automatically creates your load balancer across Availability Zones", so
    /// there is no single zone to name.
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    pub dns_name: String,
    /// The current `LoadBalancerState`.
    pub state: &'static str,
    /// The state the clock owes this load balancer, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The instance port traffic is directed to, kept as the request asked.
    pub instance_port: i32,
    pub health_check_path: String,
    pub ip_address_type: String,
    /// The TLS security policy the request named, or `None` when it named none.
    pub tls_policy_name: Option<String>,
    /// The SSL/TLS certificates this load balancer holds, in the order they
    /// were added — at most [`MAX_CERTIFICATES_PER_LOAD_BALANCER`] of them.
    pub certificates: Vec<StoredLoadBalancerCertificate>,
    /// The instances registered with this load balancer, in registration order
    /// — what `instanceHealthSummary` reports one entry per.
    pub attached_instances: Vec<StoredLoadBalancerRegistration>,
    /// Whether session stickiness is on, and for how long its cookie lives.
    ///
    /// `CreateLoadBalancer` exposes no stickiness parameter at all and its
    /// documentation says "To change additional load balancer settings, use the
    /// UpdateLoadBalancerAttribute operation", so a load balancer is created
    /// without stickiness and that operation is what turns it on.
    pub session_stickiness_enabled: bool,
    /// See [`DEFAULT_SESSION_STICKINESS_COOKIE_DURATION_SECONDS`].
    pub session_stickiness_cookie_duration_seconds: i64,
    /// Whether HTTP requests are redirected to HTTPS. Off on a load balancer
    /// this call just created, for the same reason stickiness is: it is an
    /// `UpdateLoadBalancerAttribute` setting, and there is no HTTPS listener to
    /// redirect to until a certificate is attached.
    pub https_redirection_enabled: bool,
}

/// ONE of a load balancer's configuration options, with the value already
/// resolved out of the `attributeValue` string the request carried.
///
/// The modelled `attributeValue` is a string whatever the attribute is, and each
/// `LoadBalancerAttributeName` documents its own value domain — a ping path, a
/// boolean, a number of seconds, a published TLS policy name. This is what a
/// value of one of those domains looks like once it has been read out of that
/// string, so the store never holds a value the attribute does not admit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadBalancerAttribute {
    /// `HealthCheckPath` — "the path to ping on the target".
    HealthCheckPath(String),
    /// `SessionStickinessEnabled` — whether session stickiness is active.
    SessionStickinessEnabled(bool),
    /// `SessionStickiness_LB_CookieDurationSeconds` — "the cookie duration in
    /// seconds".
    SessionStickinessCookieDurationSeconds(i64),
    /// `HttpsRedirectionEnabled` — whether HTTP to HTTPS redirection is active.
    HttpsRedirectionEnabled(bool),
    /// `TlsPolicyName` — "the name of the TLS policy".
    TlsPolicyName(String),
}

impl StoredLoadBalancer {
    /// Whether an SSL/TLS certificate is ATTACHED to this load balancer, which
    /// is what decides its protocol and its public ports.
    pub(super) fn serves_https(&self) -> bool {
        self.certificates
            .iter()
            .any(|certificate| certificate.is_attached)
    }

    /// Register the instance `name` with this load balancer.
    ///
    /// An instance is registered with a load balancer at most once — the
    /// modelled `instanceHealthSummary` carries one entry per registered
    /// instance — so registering one that is already registered leaves its
    /// place in the registration order as it is. A registration that was
    /// DRAINING stops draining: the instance the deregistration was going to
    /// remove is one this call asks the load balancer to serve again.
    pub fn register_instance(&mut self, name: &str) {
        match self
            .attached_instances
            .iter_mut()
            .find(|registration| registration.name == name)
        {
            Some(registration) => registration.drains_at_ms = None,
            None => self.attached_instances.push(StoredLoadBalancerRegistration {
                name: name.to_string(),
                drains_at_ms: None,
            }),
        }
    }

    /// Begin the deregistration of the instance `name`, due at `at_ms`, and
    /// answer the instant its removal is owed — or `None` when that instance is
    /// not registered with this load balancer and there is nothing to remove.
    ///
    /// A deregistration already in flight keeps the instant it was already owed
    /// rather than pushing it further out, so repeating the call cannot hold an
    /// instance registered for longer.
    pub fn start_deregistration(&mut self, name: &str, at_ms: i64) -> Option<i64> {
        let registration = self
            .attached_instances
            .iter_mut()
            .find(|registration| registration.name == name)?;
        Some(*registration.drains_at_ms.get_or_insert(at_ms))
    }

    /// The certificate `name` names among the ones this load balancer holds.
    ///
    /// A load balancer's certificate is addressed by its own name among THIS
    /// load balancer's certificates, which is how every certificate operation
    /// reaches one: the same name may name a certificate of another load
    /// balancer, and the two are different records.
    pub fn certificate(&self, name: &str) -> Option<&StoredLoadBalancerCertificate> {
        self.certificate_index(name)
            .and_then(|index| self.certificates.get(index))
    }

    /// WHERE the certificate `name` names sits among the ones this load
    /// balancer holds — what an operation that REMOVES one needs. The one
    /// lookup rule, shared with [`Self::certificate`].
    pub fn certificate_index(&self, name: &str) -> Option<usize> {
        self.certificates
            .iter()
            .position(|certificate| certificate.name == name)
    }

    /// Set ONE of this load balancer's configuration options to `attribute`.
    ///
    /// The counterpart of [`Self::configuration_options`]: every
    /// `LoadBalancerAttributeName` this record reports a value for is written
    /// here and read there, so the option a caller sets is the one the next read
    /// answers. "You can only update one attribute at a time", which is why this
    /// takes exactly one.
    pub fn set_attribute(&mut self, attribute: LoadBalancerAttribute) {
        match attribute {
            LoadBalancerAttribute::HealthCheckPath(path) => self.health_check_path = path,
            LoadBalancerAttribute::SessionStickinessEnabled(enabled) => {
                self.session_stickiness_enabled = enabled;
            }
            LoadBalancerAttribute::SessionStickinessCookieDurationSeconds(seconds) => {
                self.session_stickiness_cookie_duration_seconds = seconds;
            }
            LoadBalancerAttribute::HttpsRedirectionEnabled(enabled) => {
                self.https_redirection_enabled = enabled;
            }
            LoadBalancerAttribute::TlsPolicyName(name) => self.tls_policy_name = Some(name),
        }
    }

    /// Make the certificate `name` the one this load balancer terminates HTTPS
    /// with — the ONE owner of that rule, as
    /// [`ComAmazonawsLightsailLightsail20161128State::issued_certificate`] is for
    /// the certificates of the account.
    ///
    /// A load balancer terminates HTTPS with ONE certificate, so attaching one
    /// detaches whichever was attached: "Use the AttachLoadBalancerTlsCertificate
    /// action with the non-attached certificate, and it will replace the existing
    /// one and become the attached certificate" (the modelled
    /// `AttachLoadBalancerTlsCertificate` documentation). Attaching the one
    /// already attached leaves the record in the state the request asks for.
    ///
    /// The certificate has to be one this load balancer holds AND one the
    /// certificate authority has already issued — "Once you create and validate
    /// your certificate, you can attach it to your load balancer": a
    /// certificate still being validated secures nothing, so there is no HTTPS
    /// traffic the load balancer could terminate with it. Both are decided
    /// BEFORE a single certificate is written — the name through
    /// [`Self::certificate_index`] and the issuance through
    /// [`StoredLoadBalancerCertificate::is_issued`] — so a refused request
    /// cannot leave an attachment cleared or move one. Both walks are bounded by
    /// [`MAX_CERTIFICATES_PER_LOAD_BALANCER`].
    pub fn attach_certificate(&mut self, name: &str) -> Result<(), CertificateNotUsable> {
        let index = self
            .certificate_index(name)
            .ok_or(CertificateNotUsable::DoesNotExist)?;
        let requested = &self.certificates[index];
        if !requested.is_issued() {
            return Err(CertificateNotUsable::NotIssued {
                status: requested.status,
            });
        }
        for certificate in &mut self.certificates {
            certificate.is_attached = certificate.name == name;
        }
        Ok(())
    }

    /// Every SSL/TLS certificate this load balancer holds, as the modelled
    /// `LoadBalancerTlsCertificate`, in the order they were added — what
    /// `GetLoadBalancerTlsCertificates` reports.
    ///
    /// A certificate is reported on the load balancer that holds it, so each
    /// view carries this load balancer's name, and the location each record
    /// keeps is this load balancer's own; the walk is bounded by
    /// [`MAX_CERTIFICATES_PER_LOAD_BALANCER`].
    pub fn certificate_views(&self) -> Vec<LoadBalancerTlsCertificate> {
        self.certificates
            .iter()
            .map(|certificate| certificate.view(&self.name))
            .collect()
    }

    /// The modelled `configurationOptions` map: one entry per
    /// `LoadBalancerAttributeName` this record holds a value for.
    pub(super) fn configuration_options(&self) -> HashMap<String, String> {
        let mut options = HashMap::new();
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH.to_string(),
            self.health_check_path.clone(),
        );
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_ENABLED.to_string(),
            self.session_stickiness_enabled.to_string(),
        );
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_SESSION_STICKINESS_COOKIE_DURATION.to_string(),
            self.session_stickiness_cookie_duration_seconds.to_string(),
        );
        options.insert(
            LOAD_BALANCER_ATTRIBUTE_HTTPS_REDIRECTION_ENABLED.to_string(),
            self.https_redirection_enabled.to_string(),
        );
        if let Some(tls_policy_name) = &self.tls_policy_name {
            options.insert(
                LOAD_BALANCER_ATTRIBUTE_TLS_POLICY_NAME.to_string(),
                tls_policy_name.clone(),
            );
        }
        options
    }

    /// This record as the modelled `LoadBalancer`.
    pub fn view(&self) -> LoadBalancer {
        let serves_https = self.serves_https();
        LoadBalancer {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_LOAD_BALANCER.to_string()),
            tags: Some(self.tags.clone()),
            dns_name: Some(self.dns_name.clone()),
            state: Some(self.state.to_string()),
            protocol: Some(
                if serves_https {
                    LOAD_BALANCER_PROTOCOL_HTTP_HTTPS
                } else {
                    LOAD_BALANCER_PROTOCOL_HTTP
                }
                .to_string(),
            ),
            public_ports: Some(if serves_https {
                vec![LOAD_BALANCER_HTTP_PORT, LOAD_BALANCER_HTTPS_PORT]
            } else {
                vec![LOAD_BALANCER_HTTP_PORT]
            }),
            health_check_path: Some(self.health_check_path.clone()),
            instance_port: Some(self.instance_port),
            // One entry per registered instance, in registration order, each
            // reporting the health its own registration is in (see
            // [`StoredLoadBalancerRegistration::view`]).
            instance_health_summary: Some(
                self.attached_instances
                    .iter()
                    .map(StoredLoadBalancerRegistration::view)
                    .collect(),
            ),
            tls_certificate_summaries: Some(
                self.certificates
                    .iter()
                    .map(|certificate| LoadBalancerTlsCertificateSummary {
                        name: Some(certificate.name.clone()),
                        is_attached: Some(certificate.is_attached),
                    })
                    .collect(),
            ),
            configuration_options: Some(self.configuration_options()),
            ip_address_type: Some(self.ip_address_type.clone()),
            https_redirection_enabled: Some(self.https_redirection_enabled),
            tls_policy_name: self.tls_policy_name.clone(),
        }
    }
}

/// The `InstanceHealthState` an instance registered with a load balancer starts
/// in, from the modelled enum and the `instanceHealthReason` documentation.
pub const INSTANCE_HEALTH_INITIAL: &str = "initial";
/// See [`INSTANCE_HEALTH_INITIAL`].
pub const INSTANCE_HEALTH_REASON_REGISTRATION_IN_PROGRESS: &str = "Lb.RegistrationInProgress";

/// The `InstanceHealthState` an instance whose deregistration is outstanding is
/// in, from the modelled enum: it is still registered and still finishing the
/// requests it holds, which is what `DetachInstancesFromLoadBalancer`'s own
/// documentation describes ("This operation waits until the instances are no
/// longer needed before they are detached from the load balancer").
pub const INSTANCE_HEALTH_DRAINING: &str = "draining";
/// See [`INSTANCE_HEALTH_DRAINING`] — the modelled `InstanceHealthReason` value
/// for it.
pub const INSTANCE_HEALTH_REASON_DEREGISTRATION_IN_PROGRESS: &str =
    "Instance.DeregistrationInProgress";

/// The unit the load balancer metric `metric_name` is published in, or `None`
/// when this service publishes no such metric for a load balancer.
///
/// Every member of the modelled `LoadBalancerMetricName` enum — which is what
/// `GetLoadBalancerMetricData`'s boundary walk admits — is also a metric an alarm
/// can watch on a load balancer, and the "published `unit` value" each metric's
/// own documentation states is already the `unit` of that metric's
/// [`ALARM_METRICS`] row, so this reads that one catalog rather than restating
/// it. (`INSTANCE_METRIC_UNITS` is its own list only because an instance
/// publishes a metric no alarm can watch.) Source:
/// `smithy_query(query="doc",shape="GetLoadBalancerMetricDataRequest",member="metricName")`.
pub fn load_balancer_metric_unit(metric_name: &str) -> Option<&'static str> {
    alarm_metric(RESOURCE_TYPE_LOAD_BALANCER, metric_name).map(|spec| spec.unit)
}
