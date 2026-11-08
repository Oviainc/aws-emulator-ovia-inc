// SPDX-License-Identifier: CC0-1.0

//! Temporary access to one instance: the SSH key pair the service grants for it,
//! the Windows Administrator password it holds for it, and the host key its
//! browser-based access path has witnessed.
//!
//! Every value here is computed from real key material — the OpenSSH key formats
//! through `ssh-key`, the RDP certificate through `rcgen` — so a fingerprint is a
//! digest of a key this store generated rather than a string shaped like one.
//! Nothing is written to disk: the material lives in this store for as long as
//! the grant does, and the private half of a granted pair is handed to the caller
//! in the response alone.

use super::*;

/// One value of the modelled `InstanceAccessProtocol` enum
/// (`smithy_query(query="shape",shape="InstanceAccessProtocol")`), and the one a
/// request that names no protocol is served under: "The protocol to use to
/// connect to your instance. Defaults to ssh"
/// (`GetInstanceAccessDetailsRequest$protocol`).
pub const INSTANCE_ACCESS_PROTOCOL_SSH: &str = "ssh";

/// The other value of that enum: the Windows Remote Desktop protocol.
pub const INSTANCE_ACCESS_PROTOCOL_RDP: &str = "rdp";

/// The `HostKeyAttributes$algorithm` of a recorded RDP certificate, from that
/// member's own documentation: "For RDP certificates, the algorithm is always
/// `x509-cert`." An SSH host key's algorithm is the key's own
/// ([`witness_ssh_host_key`]), so this is the one algorithm name this module
/// states rather than reads off key material.
pub const HOST_KEY_ALGORITHM_X509_CERT: &str = "x509-cert";

/// How long the temporary SSH key pair this service grants stays usable —
/// `InstanceAccessDetails$expiresAt` is "For SSH access, the date on which the
/// temporary keys expire".
///
/// CHOSEN, not an AWS duration: Lightsail publishes no span for it, only that
/// the keys expire. An hour is the one access window this service's own model
/// does publish — the Amazon DCV session's documented hour
/// ([`GUI_SESSION_ACTIVE_MILLIS`]) — so temporary access to an instance is
/// granted for the same span rather than for a shorter one a test could wait out.
pub const TEMPORARY_SSH_KEY_VALID_MILLIS: i64 = 60 * 60 * 1_000;

/// How long the RDP certificate this service records for a Windows instance is
/// valid for, which `notValidBefore`/`notValidAfter` report ("The returned RDP
/// certificate is valid after this point in time").
///
/// CHOSEN: Lightsail publishes no validity span for the certificate it reports.
/// A year is the span the certificate is ISSUED for here, and the window reported
/// is that certificate's own, so the two cannot disagree.
pub const RDP_CERTIFICATE_VALID_MILLIS: i64 = 365 * 24 * 60 * 60 * 1_000;

/// Milliseconds in one UTC day, the grid a certificate's validity window is cut
/// on: an X.509 validity is a calendar instant, and the one this module issues is
/// the start of the day the certificate was witnessed on.
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;

/// How many characters the Windows Administrator password this service holds for
/// an instance carries.
///
/// CHOSEN: Lightsail publishes no length for the password it generates for a
/// Windows instance, only that the password it returns for RDP access is the
/// instance's own. Sixteen characters of [`ADMINISTRATOR_PASSWORD_ALPHABET`] is
/// above the fourteen characters Windows' own "complex password" policy asks for,
/// so a held password satisfies the policy the instance it belongs to enforces.
pub const ADMINISTRATOR_PASSWORD_LEN: usize = 16;

/// The characters an Administrator password is drawn from: ASCII letters, digits
/// and the punctuation a Windows password accepts, with no space — a space is
/// legal in a Windows password but is lost by a client that trims what it pastes.
const ADMINISTRATOR_PASSWORD_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!#$%&()*+,-.:;<=>?@[]^_{|}~";

/// The `key_id` the temporary access certificate carries: what the certificate
/// was issued for, which is how an OpenSSH server's log names it.
const TEMPORARY_SSH_KEY_ID: &str = "lightsail-instance-access";

/// The temporary SSH key pair this service has granted for one instance.
///
/// The PRIVATE half is held because the grant outlives the call that made it: the
/// same pair is reported again for as long as it is live, so a caller that asks
/// twice is not handed a second key that silently replaces the first. It is never
/// written to disk and never logged — see this type's `Debug`, which redacts it.
#[derive(Clone)]
pub struct TemporarySshKeys {
    /// When the pair was granted.
    pub created_at_ms: i64,
    /// The instant the pair stops being usable, which `expiresAt` reports and the
    /// certificate's own validity ends at.
    pub expires_at_ms: i64,
    /// The private half, in the OpenSSH private key form an OpenSSH client reads
    /// — `InstanceAccessDetails$privateKey`, "the temporary private key".
    pub private_key: String,
    /// The OpenSSH CERTIFICATE issued over the public half, which is what
    /// `certKey` is: "the public key to use when accessing your instance … you
    /// should save this value to `tempkey-cert.pub`", the file name an OpenSSH
    /// client reads a certificate from.
    ///
    /// A certificate names the user it is good for, so it is issued only for an
    /// instance whose blueprint this service publishes a login user for; for any
    /// other blueprint this carries the public half itself, which is the rest of
    /// what that member documents.
    pub cert_key: String,
}

/// Redacts the private half: a derived `Debug` would carry the granted key into
/// any formatter that renders a stored instance, including a log line.
impl std::fmt::Debug for TemporarySshKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemporarySshKeys")
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("cert_key", &self.cert_key)
            .finish_non_exhaustive()
    }
}

impl TemporarySshKeys {
    /// Grant a temporary pair at `now_ms`, valid for
    /// [`TEMPORARY_SSH_KEY_VALID_MILLIS`], whose certificate names `principal` as
    /// the user it may sign in as.
    ///
    /// A certificate states the user it is good for, so a blueprint this service
    /// publishes no login user for is granted the pair with its public half as
    /// `certKey` rather than a certificate naming a user nothing published.
    ///
    /// `None` only when the key material does not encode, which is what a caller
    /// reports no key material for rather than inventing one.
    pub fn grant(now_ms: i64, principal: Option<&str>) -> Option<Self> {
        let key = fresh_ed25519_key();
        let expires_at_ms = now_ms.saturating_add(TEMPORARY_SSH_KEY_VALID_MILLIS);
        let cert_key = match principal {
            Some(principal) => {
                temporary_access_certificate(&key, principal, now_ms, expires_at_ms)?
            }
            None => key.public_key().to_openssh().ok()?,
        };
        let private_key = key
            .to_openssh(::ssh_key::LineEnding::LF)
            .ok()?
            .to_string();
        Some(Self {
            created_at_ms: now_ms,
            expires_at_ms,
            private_key,
            cert_key,
        })
    }

    /// Whether this grant is still usable at `now_ms` — an expired one is
    /// replaced by a fresh grant rather than reported again.
    pub fn is_live_at(&self, now_ms: i64) -> bool {
        now_ms < self.expires_at_ms
    }
}

/// The temporary access this service holds for one instance.
///
/// It hangs off the instance's own record, as its GUI session access does: access
/// is access TO that instance, so deleting the instance takes the grant with it.
#[derive(Clone, Default)]
pub struct StoredInstanceAccess {
    /// The temporary SSH key pair granted for the instance, once one has been.
    pub ssh: Option<TemporarySshKeys>,
    /// The Windows Administrator password the service holds for the instance,
    /// once the instance has been ready for one to be generated.
    ///
    /// Held rather than re-generated per call because the password IS the
    /// instance's: "If you change the Administrator password on the instance,
    /// Lightsail will continue to return the original password value"
    /// (`InstanceAccessDetails$password`).
    pub administrator_password: Option<String>,
}

/// Redacts the held password, for the reason [`TemporarySshKeys`]' own `Debug`
/// redacts the private key.
impl std::fmt::Debug for StoredInstanceAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredInstanceAccess")
            .field("ssh", &self.ssh)
            .field(
                "administrator_password",
                &self.administrator_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The value `password` carries when the instance's password cannot be reported:
/// "Password will be an empty string if the password for your new instance is not
/// ready yet", and "If you create an instance using any key pair other than the
/// default (`LightsailDefaultKeyPair`), `password` will always be an empty
/// string" — both from `InstanceAccessDetails$password`.
pub const PASSWORD_NOT_AVAILABLE: &str = "";

/// The host key an SSH access to an instance witnesses: a real Ed25519 SSH host
/// key, described by the crate that implements the OpenSSH key formats.
///
/// `publicKey` is the key BLOB — the base64 body of the OpenSSH public key line,
/// which is what Lightsail reports and what a `known_hosts` entry carries beside
/// the algorithm. The fingerprints are OpenSSH's own: `SHA256:<base64>` as
/// `ssh-keygen -l` prints it, and the same encoding over a SHA-1 digest, the two
/// forms `HostKeyAttributes$fingerprintSHA1`/`fingerprintSHA256` show for an SSH
/// host key.
///
/// `notValidBefore`/`notValidAfter` are absent: "This value is listed only for
/// RDP certificates" (`HostKeyAttributes$notValidBefore`).
pub fn witness_ssh_host_key(now_ms: i64) -> Option<HostKeyAttributes> {
    let key = fresh_ed25519_key();
    let public = key.public_key();
    let openssh = public.to_openssh().ok()?;
    let body = openssh_key_body(&openssh)?;
    Some(HostKeyAttributes {
        algorithm: Some(public.algorithm().as_str().to_string()),
        public_key: Some(body.to_string()),
        witnessed_at: Some(timestamp(now_ms)),
        fingerprint_sha1: openssh_sha1_fingerprint(body),
        fingerprint_sha256: Some(public.fingerprint(::ssh_key::HashAlg::Sha256).to_string()),
        not_valid_before: None,
        not_valid_after: None,
    })
}

/// The certificate an RDP access to the Windows instance `instance_name`
/// witnesses: a real self-signed X.509 certificate issued to that instance,
/// valid from the day it was witnessed for [`RDP_CERTIFICATE_VALID_MILLIS`].
///
/// `publicKey` is the certificate itself, PEM-encoded ("The public SSH host key
/// or the RDP certificate"), and the fingerprints are digests of its DER in the
/// colon-separated hex form `fingerprintSHA1`/`fingerprintSHA256` show for an RDP
/// certificate.
///
/// `None` when the certificate does not issue, which a caller records no host key
/// for rather than reporting a certificate nothing generated.
pub fn witness_rdp_certificate(instance_name: &str, now_ms: i64) -> Option<HostKeyAttributes> {
    let not_valid_before_ms = start_of_utc_day(now_ms);
    let not_valid_after_ms =
        start_of_utc_day(now_ms.saturating_add(RDP_CERTIFICATE_VALID_MILLIS));
    let (from_year, from_month, from_day) = utc_year_month_day(not_valid_before_ms)?;
    let (to_year, to_month, to_day) = utc_year_month_day(not_valid_after_ms)?;

    let key = ::rcgen::KeyPair::generate().ok()?;
    // No subject alternative name: a Lightsail instance name admits characters a
    // DNS name does not (`^\w[\w\-]*\w$` allows `_`), so the instance is named in
    // the subject's common name — as a Windows machine's own Remote Desktop
    // certificate names the machine — rather than in a SAN that would refuse it.
    let mut params = ::rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, instance_name);
    params.distinguished_name = subject;
    params.not_before = ::rcgen::date_time_ymd(from_year, from_month, from_day);
    params.not_after = ::rcgen::date_time_ymd(to_year, to_month, to_day);
    let certificate = params.self_signed(&key).ok()?;
    let der = certificate.der().as_ref().to_vec();

    Some(HostKeyAttributes {
        algorithm: Some(HOST_KEY_ALGORITHM_X509_CERT.to_string()),
        public_key: Some(certificate.pem()),
        witnessed_at: Some(timestamp(now_ms)),
        fingerprint_sha1: Some(colon_hex(&::sha1::Sha1::digest(&der))),
        fingerprint_sha256: Some(colon_hex(&Sha256::digest(&der))),
        not_valid_before: Some(timestamp(not_valid_before_ms)),
        not_valid_after: Some(timestamp(not_valid_after_ms)),
    })
}

/// A fresh Ed25519 key, the one kind of key material this module generates: the
/// algorithm OpenSSH generates by default, and the one `ssh-key` implements
/// without a native dependency.
fn fresh_ed25519_key() -> ::ssh_key::PrivateKey {
    ::ssh_key::PrivateKey::from(::ssh_key::private::Ed25519Keypair::random(
        &mut ::ssh_key::rand_core::OsRng,
    ))
}

/// The key this emulator issues its temporary access certificates under.
///
/// A certificate authority belongs to the SERVICE rather than to an account's
/// resource, so it is drawn once per process — like [`process_seed`] — and never
/// held in a store an operation can read it back out of. Nothing here can present
/// it: the emulator serves no SSH transport.
fn access_certificate_authority() -> &'static ::ssh_key::PrivateKey {
    static AUTHORITY: OnceLock<::ssh_key::PrivateKey> = OnceLock::new();
    AUTHORITY.get_or_init(fresh_ed25519_key)
}

/// The OpenSSH certificate over `key`'s public half, valid from `valid_after_ms`
/// until `valid_before_ms` and naming `principal` as the user it signs in as.
///
/// This is what `certKey` carries, so the expiry a caller is told is the one the
/// certificate itself bears rather than a second copy of it.
fn temporary_access_certificate(
    key: &::ssh_key::PrivateKey,
    principal: &str,
    valid_after_ms: i64,
    valid_before_ms: i64,
) -> Option<String> {
    let valid_after = u64::try_from(valid_after_ms.div_euclid(1_000)).ok()?;
    let valid_before = u64::try_from(valid_before_ms.div_euclid(1_000)).ok()?;
    let mut builder = ::ssh_key::certificate::Builder::new_with_random_nonce(
        &mut ::ssh_key::rand_core::OsRng,
        key.public_key(),
        valid_after,
        valid_before,
    )
    .ok()?;
    builder.key_id(TEMPORARY_SSH_KEY_ID).ok()?;
    // The user the certificate is good for, which is the user the instance's own
    // blueprint signs in as.
    builder.valid_principal(principal).ok()?;
    builder
        .sign(access_certificate_authority())
        .ok()?
        .to_openssh()
        .ok()
}

/// The base64 body of an OpenSSH public key line, which is laid out as
/// `<algorithm> <body>[ <comment>]`.
fn openssh_key_body(openssh: &str) -> Option<&str> {
    openssh.split_whitespace().nth(1)
}

/// OpenSSH's SHA-1 fingerprint of the public key whose blob `body` is the base64
/// of: `SHA1:` and the unpadded base64 of the digest, the form
/// `fingerprintSHA1`'s own example shows for an SSH host key.
fn openssh_sha1_fingerprint(body: &str) -> Option<String> {
    use ::base64::Engine;
    let blob = ::base64::engine::general_purpose::STANDARD.decode(body).ok()?;
    let digest = ::sha1::Sha1::digest(&blob);
    Some(format!(
        "SHA1:{}",
        ::base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    ))
}

/// `bytes` as the colon-separated lowercase hex an X.509 certificate's
/// fingerprint is published in, the form `fingerprintSHA1`'s own RDP example
/// shows.
fn colon_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The start of the UTC day `epoch_millis` falls in.
fn start_of_utc_day(epoch_millis: i64) -> i64 {
    epoch_millis - epoch_millis.rem_euclid(MILLIS_PER_DAY)
}

/// The UTC calendar date of `epoch_millis` as `(year, month, day)`.
///
/// The calendar conversion is [`utc_date`]'s — the timestamp library's — so no
/// leap-year arithmetic is re-derived here.
fn utc_year_month_day(epoch_millis: i64) -> Option<(i32, u8, u8)> {
    let date = utc_date(epoch_millis);
    let mut fields = date.split('-');
    let year = fields.next()?.parse().ok()?;
    let month = fields.next()?.parse().ok()?;
    let day = fields.next()?.parse().ok()?;
    Some((year, month, day))
}

impl ComAmazonawsLightsailLightsail20161128State {
    /// Grant access to the instance `name` names over `protocol`, answering what
    /// a caller is told about it — or `None` when no instance of that name is
    /// stored, in which case nothing was written and no identifier was spent.
    ///
    /// The presence check comes BEFORE anything is minted, for the reason
    /// [`Self::create_gui_session`]'s does: a call that resolves no instance must
    /// not consume a key, a password or an id.
    ///
    /// What the call grants depends on the protocol and on the instance:
    ///
    /// * an SSH access is granted a temporary key pair, and the SAME pair for as
    ///   long as that grant is live — the keys are the resource this call creates,
    ///   so asking twice inside the window does not silently retire the pair the
    ///   first answer handed out;
    /// * an RDP access to a WINDOWS instance is told the instance's Administrator
    ///   password, or the data to decrypt it, under the conditions
    ///   `InstanceAccessDetails$password` and `$passwordData` state. An instance
    ///   of any other platform is told neither: both members are documented "for
    ///   a Windows Server-based instance";
    /// * either way the host key the instance's own access service presents is
    ///   RECORDED on the instance and reported from then on — "the time that the
    ///   SSH host key or RDP certificate was recorded by Lightsail"
    ///   (`HostKeyAttributes$witnessedAt`) — so two accesses to one instance are
    ///   authenticated by the same key. Which key that is follows the instance's
    ///   PLATFORM, because that is what decides the service listening on it: the
    ///   instance's SSH host key on a Linux/Unix instance, its RDP certificate on
    ///   a Windows one. `DeleteKnownHostKeys` is what empties the record, and the
    ///   next access witnesses the key afresh, which is how a caller recovers
    ///   from a host key mismatch.
    ///
    /// A key is witnessed only while the instance is RUNNING: it is the key the
    /// instance itself presents, and an instance that is still coming up or is
    /// stopped presents none, so nothing is recorded for it and the record it
    /// already holds is reported unchanged.
    pub fn grant_instance_access(
        &mut self,
        name: &str,
        protocol: &str,
        now_ms: i64,
    ) -> Option<crate::ops::types::InstanceAccessDetails> {
        let instance = self.instances.get(name)?;
        let rdp = protocol == INSTANCE_ACCESS_PROTOCOL_RDP;
        let is_running = instance.state == RUNNING;
        let is_windows = blueprint_is_windows(&instance.blueprint_id);
        let username = blueprint_username(&instance.blueprint_id).map(str::to_string);
        let ip_address = instance.public_ip_address.clone();
        let ipv6_addresses = instance.ipv6_addresses.clone();
        let key_pair_name = instance.ssh_key_name.clone();
        let is_default_key_pair = key_pair_name == DEFAULT_SSH_KEY_NAME;
        // The instance presents ONE host key, so a record it already holds is the
        // one reported again: the list a long-lived instance accumulates is
        // bounded however many times it is accessed.
        let already_witnessed = !instance.known_host_keys.is_empty();
        let held_ssh = instance.ssh_access_live_at(now_ms);
        let held_password = instance.access.administrator_password.clone();

        // Everything that MINTS runs before the record is reached for writing,
        // so the store is not left holding a grant whose material did not encode.
        let witnessed = match (is_running && !already_witnessed, is_windows) {
            (false, _) => None,
            (true, true) => witness_rdp_certificate(name, now_ms),
            (true, false) => witness_ssh_host_key(now_ms),
        };
        let granted_ssh = match (rdp, held_ssh) {
            // An RDP access is granted no SSH key pair: `privateKey`, `certKey`
            // and `expiresAt` are all documented "for SSH access".
            (true, _) => None,
            (false, Some(held)) => Some(held),
            (false, None) => TemporarySshKeys::grant(now_ms, username.as_deref()),
        };
        // The password the service holds for the instance: generated the first
        // time an RDP access finds the instance ready — "it can take up to 15
        // minutes for the instance to be ready" — and the same one from then on.
        // It is held only where it can be handed out, which is an instance
        // launched with the default key pair: with any other pair the password is
        // documented to be reported empty and only its ciphertext is of use, and
        // that ciphertext is the pair's to make (see `password_data` below).
        let held_or_minted = (rdp && is_windows && is_running && is_default_key_pair).then(|| {
            held_password.unwrap_or_else(|| {
                self.mint_token(
                    b"instance-administrator-password",
                    ADMINISTRATOR_PASSWORD_LEN,
                    ADMINISTRATOR_PASSWORD_ALPHABET,
                )
            })
        });
        // An RDP access to a Windows instance is always told a password, which is
        // the empty string wherever the instance's own cannot be handed out.
        let password = (rdp && is_windows).then(|| {
            held_or_minted
                .clone()
                .unwrap_or_else(|| PASSWORD_NOT_AVAILABLE.to_string())
        });

        let instance = self.instances.get_mut(name)?;
        if let Some(witnessed) = witnessed {
            instance.known_host_keys.push(witnessed);
        }
        if let Some(granted) = granted_ssh.clone() {
            instance.access.ssh = Some(granted);
        }
        if let Some(generated) = held_or_minted {
            instance.access.administrator_password = Some(generated);
        }
        let host_keys = instance.known_host_keys.clone();

        // What a caller may decrypt the password with, and nothing else: the
        // member is "only needed if `password` is empty", so an access that is
        // told the password is not told this as well.
        let password_data = password
            .as_deref()
            .filter(|password| password.is_empty())
            .map(|_| crate::ops::types::PasswordData {
                // "Lightsail creates the ciphertext by encrypting your password
                // with the public key part of this key pair", so the ciphertext
                // exists only where that public half does. This store holds a key
                // pair's fingerprint and not its public key material, so a custom
                // pair's ciphertext is OMITTED rather than filled with something
                // nothing encrypted; for an instance that is not ready yet the
                // empty string is the documented value ("Ciphertext will be an
                // empty string if access to your new instance is not ready yet").
                ciphertext: (!is_running).then(|| PASSWORD_NOT_AVAILABLE.to_string()),
                key_pair_name: Some(key_pair_name),
            });

        Some(crate::ops::types::InstanceAccessDetails {
            instance_name: Some(name.to_string()),
            protocol: Some(protocol.to_string()),
            username,
            ip_address,
            ipv6_addresses: Some(ipv6_addresses),
            host_keys: Some(host_keys),
            cert_key: granted_ssh.as_ref().map(|keys| keys.cert_key.clone()),
            private_key: granted_ssh.as_ref().map(|keys| keys.private_key.clone()),
            expires_at: granted_ssh
                .as_ref()
                .map(|keys| timestamp(keys.expires_at_ms)),
            password,
            password_data,
        })
    }
}

impl StoredInstance {
    /// The temporary SSH key pair this instance holds, if one is granted and
    /// still live at `now_ms`.
    pub fn ssh_access_live_at(&self, now_ms: i64) -> Option<TemporarySshKeys> {
        self.access
            .ssh
            .clone()
            .filter(|keys| keys.is_live_at(now_ms))
    }
}
