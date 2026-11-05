// SPDX-License-Identifier: CC0-1.0

//! The managed relational database this service runs for the account: its
//! endpoint and credentials, the windows it is maintained in, the events it
//! logs, and the snapshots taken of it.

use super::*;

/// The `parameterApplyStatus` a database whose parameters are all applied
/// reports.
///
/// A Lightsail managed database is an Amazon RDS DB instance, and the member
/// carries RDS's own parameter group status: `in-sync` means "the parameters are
/// applied"; `UpdateRelationalDatabaseParameters` is what moves a database out of
/// it.
pub const PARAMETER_APPLY_STATUS_IN_SYNC: &str = "in-sync";

/// The certificate authority a new database's server certificate is issued
/// under, as `caCertificateIdentifier` reports it.
///
/// `rds-ca-rsa2048-g1` is the default certificate authority for a new DB
/// instance ("Using SSL/TLS to encrypt a connection to a DB instance", Amazon
/// RDS User Guide). It is a value AWS allocated — nothing computes it — so a
/// single published name is the only honest source; it moves when the published
/// default does.
pub const RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIER: &str = "rds-ca-rsa2048-g1";

/// The certificate authorities a database's server certificate can be issued
/// under — the identifiers `caCertificateIdentifier` may name.
///
/// Source: the certificate authorities "Using SSL/TLS to encrypt a connection to
/// a DB instance" (Amazon RDS User Guide) publishes for a DB instance — the
/// 2019 authority and the three current ones, which differ in the key the
/// certificate carries (RSA 2048, RSA 4096 and ECC P-384). Each is a value AWS
/// allocated, so the published list is the only honest source; an authority AWS
/// adds or retires is a row added or removed here.
pub const RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS: &[&str] = &[
    "rds-ca-2019",
    "rds-ca-rsa2048-g1",
    "rds-ca-rsa4096-g1",
    "rds-ca-ecc384-g1",
];

/// The published certificate authority `identifier` names, compared exactly —
/// an identifier is a name AWS allocated, not a presentation form.
pub fn relational_database_ca_certificate(identifier: &str) -> Option<&'static str> {
    RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS
        .iter()
        .copied()
        .find(|published| *published == identifier)
}

/// The identifiers [`relational_database_ca_certificate`] accepts, for a refusal
/// that names them.
pub fn relational_database_ca_certificate_identifiers() -> String {
    RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIERS.join(", ")
}

/// What `PendingModifiedRelationalDatabaseValues$masterUserPassword` reports
/// while a password change is waiting for the database's next maintenance
/// window.
///
/// The master user password is a secret, and the operation that answers with one
/// is `GetRelationalDatabaseMasterUserPassword`; a database read may say only
/// THAT a new password is pending, never what it is. Amazon RDS masks the same
/// member of its own `PendingModifiedValues` the same way.
pub const MASKED_MASTER_USER_PASSWORD: &str = "****";

/// The DNS suffix a Lightsail managed database's master endpoint carries.
///
/// A Lightsail managed database IS an Amazon RDS DB instance — which is why
/// every member of the modelled `RelationalDatabase` is an RDS one — and AWS
/// publishes its `masterEndpoint.address` under its own
/// `rds.amazonaws.com` zone. That HOSTNAME is environment-specific and no
/// caller may assume it, while the PROMISE the member makes is not:
/// `RelationalDatabaseEndpoint$address` is "the DNS address of the database",
/// so it must never be an address that sends the next client OUT of the
/// emulator — the rule this crate already states for `Bucket$url` and a
/// container service's public address. This emulator owns no DNS zone, so the
/// address keeps the RDS SHAPE (`<db instance identifier>.<host label>.<region>`)
/// under a suffix that belongs to the emulator and cannot resolve off-machine,
/// exactly as a container service's private domain name does
/// (`PRIVATE_DOMAIN_SUFFIX`, `<service name>.service.local`): `.local` is
/// reserved for link-local multicast name resolution (RFC 6762 §3), so a name
/// under it can never address a host off this machine. The labels are
/// the database's own, so two databases never share an address.
///
/// This emulator serves no MySQL or PostgreSQL wire protocol on it, because the
/// core's native listener (`engine.listeners().tcp`) is TLS-from-the-first-byte
/// and neither engine's protocol begins with a TLS handshake: a MySQL server
/// sends its Initial Handshake Packet in the clear and only then upgrades, on
/// the client's SSL-request packet, and PostgreSQL negotiates the same way
/// through its SSLRequest message — so the core's `rustls` acceptor fails the
/// handshake of every real database driver before the connection ever reaches a
/// handler (`vendor/server/src/listeners.rs`: "TLS-only ... a plaintext/HTTP
/// preface also fails the handshake here and never reaches the handler"), and a
/// service may not bind a socket of its own. Serving a database data plane
/// needs a core seam that hands over a connection WITHOUT terminating TLS first
/// (so the protocol server can do the in-band upgrade itself); until the core
/// offers one, no address this service could announce would answer the MySQL or
/// PostgreSQL protocol. Everything the control plane owns — resolving the
/// database, its state, its hardware, its credentials and its windows — is real.
pub(super) const RELATIONAL_DATABASE_ENDPOINT_SUFFIX: &str = "rds.lightsail.local";

/// The prefix of the RDS DB instance identifier Lightsail gives a managed
/// database: Lightsail names the underlying instance after itself rather than
/// after the Lightsail resource, which is why a master endpoint begins `ls-`
/// instead of with the database's own name.
pub(super) const RELATIONAL_DATABASE_INSTANCE_ID_PREFIX: &str = "ls-";

/// How many characters the minted host label of a master endpoint carries: the
/// 12-character DNS label RDS puts between the instance identifier and the
/// region.
pub(super) const RELATIONAL_DATABASE_HOST_LABEL_LEN: usize = 12;

/// The alphabet that host label is drawn from. A DNS label is
/// case-insensitive and RDS renders this one in lowercase.
pub(super) const RELATIONAL_DATABASE_HOST_LABEL_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Whether `engine` is the PostgreSQL engine FAMILY.
///
/// The ONE engine classification every per-engine helper here shares (the port,
/// the parameter catalog, the log streams and the master-credential limits), so
/// they cannot drift apart.
///
/// It answers true when any SEPARATOR-DELIMITED token of the value is
/// `postgres` or `postgresql`, case-insensitively — which covers the catalog's
/// own [`RelationalDatabaseBlueprintSpec::engine`] value (`postgres`) and a
/// blueprint id that carries the version beside it (`postgres_17`), the two
/// forms this crate passes. An unseparated spelling (`postgres17`) is NOT
/// PostgreSQL to this predicate; everything that is not PostgreSQL is MySQL, the
/// engine the modelled `RelationalDatabaseEngine` enum names.
pub fn is_postgres_engine(engine: &str) -> bool {
    engine
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            token.eq_ignore_ascii_case("postgres") || token.eq_ignore_ascii_case("postgresql")
        })
}

/// The port a database of `engine` listens on: the engine's own default
/// (MySQL `3306`, PostgreSQL `5432`), the ports RDS documents as the default for
/// each engine under `CreateDBInstance$Port`.
///
/// Lightsail exposes no port parameter at all, so a database always listens on
/// its engine's default.
pub fn relational_database_port(engine: &str) -> i32 {
    if is_postgres_engine(engine) {
        5432
    } else {
        3306
    }
}

/// The log streams a MySQL managed database publishes, in the order
/// [`relational_database_log_streams`] reports them.
///
/// A Lightsail managed database is an Amazon RDS DB instance, and a log stream is
/// one of the log FILES that instance's engine writes: the RDS MySQL log-file
/// names (Amazon RDS User Guide, "MySQL database log files" — the error log
/// `error/mysql-error.log` with its hourly-rotated running copy
/// `error/mysql-error-running.log`, the general query log
/// `general/mysql-general.log` and the slow query log
/// `slowquery/mysql-slowquery.log`).
///
/// CHOSEN where AWS publishes no list: Lightsail's own API documents only that
/// `GetRelationalDatabaseLogStreams` answers "a list of available log streams"
/// and names none of them, so this is the engine's documented log-file set. The
/// hourly `error/mysql-error-running.log.<hour>` copies and the audit log are
/// left out: the first is a rotation artefact of a server that has been running
/// for hours, and the second needs an option group Lightsail does not offer.
pub const MYSQL_LOG_STREAMS: &[&str] = &[
    "error/mysql-error-running.log",
    "error/mysql-error.log",
    "general/mysql-general.log",
    "slowquery/mysql-slowquery.log",
];

/// The log streams a PostgreSQL managed database publishes. See
/// [`MYSQL_LOG_STREAMS`] for where the names come from: PostgreSQL writes ONE
/// log, `error/postgresql.log` (Amazon RDS User Guide, "PostgreSQL database log
/// files"), whose hourly-stamped copies are left out for the same reason the
/// MySQL rotations are — and because a stream name has to be stable across
/// reads.
pub const POSTGRES_LOG_STREAMS: &[&str] = &["error/postgresql.log"];

/// What the name of an ERROR log stream begins with, for both engines this
/// slice serves.
///
/// A database's log files fall in the groups the engine writes them in, and the
/// group is the first path segment of the stream name: the error log the engine
/// writes its own startup and shutdown to is under `error/` for both engines
/// ([`MYSQL_LOG_STREAMS`], [`POSTGRES_LOG_STREAMS`]), while a stream under
/// `general/` or `slowquery/` is a QUERY log — a record of the statements
/// clients ran, not of the engine itself.
pub const RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX: &str = "error/";

/// The log streams a database of `engine` publishes.
///
/// The set is the ENGINE's, like the port and the master-credential limits, so a
/// MySQL database and a PostgreSQL one answer different streams. The engine of a
/// blueprint is the catalog's own [`RelationalDatabaseBlueprintSpec::engine`]
/// value, so the PostgreSQL set is selected on that engine and every other engine
/// the catalog serves answers the MySQL set.
pub fn relational_database_log_streams(engine: &str) -> &'static [&'static str] {
    if is_postgres_engine(engine) {
        POSTGRES_LOG_STREAMS
    } else {
        MYSQL_LOG_STREAMS
    }
}

/// The ARN of the managed relational database `id` in `region`.
pub fn relational_database_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{RELATIONAL_DATABASE_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The master endpoint address of a database whose underlying DB instance was
/// minted as `id`, whose host label is `host_label`, in `region`. See
/// [`RELATIONAL_DATABASE_ENDPOINT_SUFFIX`].
pub fn relational_database_endpoint_address(id: &str, host_label: &str, region: &str) -> String {
    format!(
        "{RELATIONAL_DATABASE_INSTANCE_ID_PREFIX}{}.{host_label}.{region}.{RELATIONAL_DATABASE_ENDPOINT_SUFFIX}",
        id.replace('-', "")
    )
}

/// The Availability Zone a resource of `resource_name` is created in when the
/// request names none, out of the zones `zones` its Region publishes.
///
/// DERIVED from the resource's own name rather than drawn per call, so the zone
/// a resource was placed in is the same on every later read, and `None` when the
/// Region publishes no zone at all.
pub fn default_availability_zone(resource_name: &str, zones: &[String]) -> Option<String> {
    let digest = Sha256::digest(resource_name.as_bytes());
    let index = usize::from(digest[0]) % zones.len().max(1);
    zones.get(index).cloned()
}

/// The zone a high-availability resource's standby runs in: the zone after
/// `primary` in the Region's own zone order, wrapping — so it is never the
/// primary's own zone, which is the point of a standby.
///
/// `None` when the Region publishes fewer than two zones, and then there is no
/// second zone to fail over to.
pub fn secondary_availability_zone(primary: &str, zones: &[String]) -> Option<String> {
    if zones.len() < 2 {
        return None;
    }
    let primary_index = zones.iter().position(|zone| zone == primary)?;
    zones.get((primary_index + 1) % zones.len()).cloned()
}

/// The master-credential constraints one database engine imposes, from the
/// modelled `CreateRelationalDatabase` input documentation, which states them per
/// engine: `masterDatabaseName` ("Must contain 1 to 64 letters or numbers" for
/// MySQL, 1 to 63 for PostgreSQL), `masterUsername` ("Must be 1 to 16 letters or
/// numbers" for MySQL, 1 to 63 for PostgreSQL) and `masterUserPassword` ("Must
/// contain from 8 to 41 characters" for MySQL, from 8 to 128 for PostgreSQL).
#[derive(Debug, Clone, Copy)]
pub struct MasterCredentialLimits {
    /// The most characters `masterDatabaseName` may carry.
    pub database_name_chars: usize,
    /// The most characters `masterUsername` may carry.
    pub username_chars: usize,
    /// The fewest and the most characters `masterUserPassword` may carry.
    pub password_chars: (usize, usize),
}

/// The limits [`MasterCredentialLimits`] states for `engine`.
///
/// The engine of a blueprint is the catalog's own
/// [`RelationalDatabaseBlueprintSpec::engine`] value, so the PostgreSQL set is
/// selected on that engine and every other engine the catalog serves is held to
/// the MySQL set the modelled documentation gives first.
pub fn master_credential_limits(engine: &str) -> MasterCredentialLimits {
    if is_postgres_engine(engine) {
        MasterCredentialLimits {
            database_name_chars: 63,
            username_chars: 63,
            password_chars: (8, 128),
        }
    } else {
        MasterCredentialLimits {
            database_name_chars: 64,
            username_chars: 16,
            password_chars: (8, 41),
        }
    }
}

/// Refuse `value` unless it is an identifier a database engine accepts for
/// `member`: 1 to `max_chars` characters, beginning with a letter, the rest
/// letters, digits or underscores.
///
/// Both `masterDatabaseName` and `masterUsername` carry that same rule in the
/// modelled documentation ("Must begin with a letter. Subsequent characters can
/// be letters, underscores, or digits (0-9)"), differing only in the length each
/// engine allows. The engine's own reserved-word list is NOT checked here: it is
/// the engine's, not the service's, and this slice carries no copy of it.
pub fn validate_engine_identifier(
    member: &str,
    value: &str,
    max_chars: usize,
) -> Result<(), String> {
    let length = value.chars().count();
    if length == 0 || length > max_chars {
        return Err(format!(
            "{member} carries {length} characters, outside the 1 to {max_chars} this database \
             engine accepts"
        ));
    }
    if !value
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
    {
        return Err(format!("{member} '{value}' must begin with a letter"));
    }
    if let Some(bad) = value
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || *character == '_'))
    {
        return Err(format!(
            "{member} '{value}' carries '{bad}': after the first character only letters, digits \
             and underscores are accepted"
        ));
    }
    Ok(())
}

/// Refuse `password` unless it is a master user password this service accepts:
/// `min` to `max` characters of printable ASCII, none of them a space, a `/`, a
/// `"` or an `@`.
///
/// From the modelled `masterUserPassword` documentation: "The password can
/// include any printable ASCII character except "/", """, or "@". It cannot
/// contain spaces", with the length bounds [`MasterCredentialLimits`] carries
/// per engine.
pub fn validate_master_user_password(
    password: &str,
    (min, max): (usize, usize),
) -> Result<(), String> {
    let length = password.chars().count();
    if length < min || length > max {
        return Err(format!(
            "masterUserPassword carries {length} characters, outside the {min} to {max} this \
             database engine accepts"
        ));
    }
    if let Some(bad) = password.chars().find(|character| {
        !character.is_ascii_graphic() || matches!(character, '/' | '"' | '@')
    }) {
        return Err(format!(
            "masterUserPassword carries '{bad}': a password carries printable ASCII characters \
             other than a space, '/', '\"' and '@'"
        ));
    }
    Ok(())
}

/// How many characters the master user password
/// [`ComAmazonawsLightsailLightsail20161128State::mint_master_user_password`]
/// generates carries.
///
/// CHOSEN: Lightsail publishes no length for the "new strong password generated
/// by Lightsail" (the modelled `rotateMasterUserPassword` documentation), only
/// that it is strong. It sits inside the narrowest range any engine this service
/// serves accepts ([`master_credential_limits`]: MySQL's 8 to 41 characters),
/// so a minted password satisfies the same rule a caller-supplied one is held
/// to, whichever engine the database runs.
pub(super) const MINTED_MASTER_USER_PASSWORD_LEN: usize = 20;

/// The characters a minted master user password is drawn from: printable ASCII
/// other than a space, `/`, `"` and `@` — exactly the set
/// [`validate_master_user_password`] accepts, from the modelled
/// `masterUserPassword` documentation.
pub(super) const MINTED_MASTER_USER_PASSWORD_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!#$%&'()*+,-.:;<=>?[\\]^_`{|}~";

/// Minutes in a day, and in a week: the clock a backup (daily) and a
/// maintenance (weekly) window are measured on.
pub(super) const MINUTES_PER_DAY: i64 = 24 * 60;
/// See [`MINUTES_PER_DAY`].
pub(super) const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;

/// The fewest minutes a backup or maintenance window may span, from the modelled
/// `preferredBackupWindow` and `preferredMaintenanceWindow` documentation: "Must
/// be at least 30 minutes."
pub const MIN_WINDOW_MINUTES: i64 = 30;

/// The days a weekly maintenance window may name, from the modelled
/// `preferredMaintenanceWindow` documentation: "Valid days: Mon, Tue, Wed, Thu,
/// Fri, Sat, Sun." Index 0 is Monday, the first day that list names.
pub const MAINTENANCE_WINDOW_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// `hh24:mi` as a minute of the day, or `None` when it is not that form.
pub(super) fn minute_of_day(time: &str) -> Option<i64> {
    let (hours, minutes) = time.split_once(':')?;
    // Each component must be exactly two ASCII DIGITS. `hh24:mi` admits no
    // sign, no space and no single-digit form — and a component that is only
    // digits cannot parse to a negative number, which the bounds below would
    // otherwise let through and carry into the arithmetic.
    if !is_two_ascii_digits(hours) || !is_two_ascii_digits(minutes) {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    // Both components are bounded above, so the answer is at most 23*60+59; the
    // arithmetic is checked anyway, so no unchecked multiplication is left on a
    // path a request reaches.
    i64::from(hours)
        .checked_mul(60)?
        .checked_add(i64::from(minutes))
}

/// Whether `component` is exactly two ASCII digits — the form each component of
/// `hh24:mi` takes.
pub(super) fn is_two_ascii_digits(component: &str) -> bool {
    component.len() == 2 && component.chars().all(|digit| digit.is_ascii_digit())
}

/// A minute of the day as `hh24:mi`.
pub(super) fn render_time(minute_of_day: i64) -> String {
    let minute_of_day = minute_of_day.rem_euclid(MINUTES_PER_DAY);
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

/// The day index `day` names in [`MAINTENANCE_WINDOW_DAYS`], compared without
/// regard to case (the days are a presentation form, not an identifier).
pub(super) fn day_index(day: &str) -> Option<i64> {
    MAINTENANCE_WINDOW_DAYS
        .iter()
        .position(|name| name.eq_ignore_ascii_case(day))
        .and_then(|index| i64::try_from(index).ok())
}

/// One window as the half-open minute interval `[start, start + span)` on its
/// own clock: minutes of the day for a daily window, minutes of the week for a
/// weekly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSpan {
    /// The minute the window opens.
    pub start: i64,
    /// How many minutes it spans.
    pub span: i64,
}

/// The daily window `window` describes, or a refusal naming `member` and the
/// constraint it broke.
///
/// From the modelled `preferredBackupWindow` documentation: "Must be in the
/// hh24:mi-hh24:mi format. Example: 16:00-16:30 … Must be at least 30 minutes."
/// A window whose end precedes its start runs over midnight, which is why the
/// span is taken on the daily clock.
pub fn parse_daily_window(member: &str, window: &str) -> Result<WindowSpan, String> {
    let (start, end) = window.split_once('-').ok_or_else(|| {
        format!("{member} '{window}' is not in the hh24:mi-hh24:mi format (16:00-16:30)")
    })?;
    let start = minute_of_day(start).ok_or_else(|| {
        format!("{member} '{window}' is not in the hh24:mi-hh24:mi format (16:00-16:30)")
    })?;
    let end = minute_of_day(end).ok_or_else(|| {
        format!("{member} '{window}' is not in the hh24:mi-hh24:mi format (16:00-16:30)")
    })?;
    let span = (end - start).rem_euclid(MINUTES_PER_DAY);
    if span < MIN_WINDOW_MINUTES {
        return Err(format!(
            "{member} '{window}' spans {span} minutes, fewer than the {MIN_WINDOW_MINUTES} a \
             window must be"
        ));
    }
    Ok(WindowSpan { start, span })
}

/// The weekly window `window` describes, or a refusal naming `member` and the
/// constraint it broke.
///
/// From the modelled `preferredMaintenanceWindow` documentation: "Must be in the
/// ddd:hh24:mi-ddd:hh24:mi format. Valid days: Mon, Tue, Wed, Thu, Fri, Sat,
/// Sun. Must be at least 30 minutes. … Example: Tue:17:00-Tue:17:30".
pub fn parse_weekly_window(member: &str, window: &str) -> Result<WindowSpan, String> {
    let malformed = || {
        format!(
            "{member} '{window}' is not in the ddd:hh24:mi-ddd:hh24:mi format \
             (Tue:17:00-Tue:17:30), with a day of [{}]",
            MAINTENANCE_WINDOW_DAYS.join(", ")
        )
    };
    let (start, end) = window.split_once('-').ok_or_else(malformed)?;
    let minute_of_week = |point: &str| -> Option<i64> {
        let (day, time) = point.split_once(':')?;
        Some(day_index(day)? * MINUTES_PER_DAY + minute_of_day(time)?)
    };
    let start = minute_of_week(start).ok_or_else(malformed)?;
    let end = minute_of_week(end).ok_or_else(malformed)?;
    let span = (end - start).rem_euclid(MINUTES_PER_WEEK);
    if span < MIN_WINDOW_MINUTES {
        return Err(format!(
            "{member} '{window}' spans {span} minutes, fewer than the {MIN_WINDOW_MINUTES} a \
             window must be"
        ));
    }
    Ok(WindowSpan { start, span })
}

/// The half-open minute ranges `window` covers on the daily clock, split where
/// it runs over midnight so two windows can be compared range by range.
///
/// A window of at most a day covers at most two ranges, so the pair is returned
/// as a fixed-size array — the second entry empty (`None`) for a window that
/// does not cross midnight — rather than as an allocation.
pub(super) fn daily_ranges(window: WindowSpan) -> [Option<(i64, i64)>; 2] {
    let start = window.start.rem_euclid(MINUTES_PER_DAY);
    let span = window.span.min(MINUTES_PER_DAY);
    if start + span <= MINUTES_PER_DAY {
        [Some((start, start + span)), None]
    } else {
        [
            Some((start, MINUTES_PER_DAY)),
            Some((0, start + span - MINUTES_PER_DAY)),
        ]
    }
}

/// Whether the backup window `backup` and the maintenance window `maintenance`
/// conflict, which the modelled `preferredBackupWindow` documentation forbids
/// ("Must not conflict with the preferred maintenance window").
///
/// A backup window recurs EVERY day and a maintenance window on one day of the
/// week, so they conflict when they overlap in time of day — whichever day the
/// maintenance window names.
pub fn windows_conflict(backup: WindowSpan, maintenance: WindowSpan) -> bool {
    let maintenance = WindowSpan {
        start: maintenance.start.rem_euclid(MINUTES_PER_DAY),
        span: maintenance.span,
    };
    let maintenance_ranges = daily_ranges(maintenance);
    daily_ranges(backup)
        .into_iter()
        .flatten()
        .any(|(backup_lo, backup_hi)| {
            maintenance_ranges
                .into_iter()
                .flatten()
                .any(|(lo, hi)| backup_lo < hi && lo < backup_hi)
        })
}

/// How many 30-minute slots a day holds, the candidates a defaulted window is
/// drawn from.
pub(super) const WINDOW_SLOTS_PER_DAY: i64 = MINUTES_PER_DAY / MIN_WINDOW_MINUTES;

/// How many slots the 8-hour block a defaulted window is selected from holds.
pub(super) const WINDOW_SLOTS_PER_BLOCK: i64 = 8 * 60 / MIN_WINDOW_MINUTES;

/// The daily backup window a database gets when the request names none, as
/// `hh24:mi-hh24:mi`.
///
/// From the modelled `preferredBackupWindow` documentation: "The default is a
/// 30-minute window selected at random from an 8-hour block of time for each AWS
/// Region." The slot is DERIVED from the database's own name rather than drawn
/// per call, so every later read of the database reports the same window, and it
/// is taken from the first 8-hour block of the day so the defaults of the two
/// windows cannot conflict. When `avoid` is a maintenance window the caller
/// named, the slot walks forward in 30-minute steps until it clears that window;
/// `None` when the caller's window leaves no slot free at all.
pub fn default_preferred_backup_window(
    resource_name: &str,
    avoid: Option<WindowSpan>,
) -> Option<String> {
    let digest = Sha256::digest(resource_name.as_bytes());
    let slot = i64::from(digest[1]) % WINDOW_SLOTS_PER_BLOCK;
    (0..WINDOW_SLOTS_PER_DAY).find_map(|step| {
        let start = (slot + step).rem_euclid(WINDOW_SLOTS_PER_DAY) * MIN_WINDOW_MINUTES;
        let candidate = WindowSpan {
            start,
            span: MIN_WINDOW_MINUTES,
        };
        let clear = avoid.is_none_or(|avoid| !windows_conflict(candidate, avoid));
        clear.then(|| {
            format!(
                "{}-{}",
                render_time(start),
                render_time(start + MIN_WINDOW_MINUTES)
            )
        })
    })
}

/// The weekly maintenance window a database gets when the request names none, as
/// `ddd:hh24:mi-ddd:hh24:mi`.
///
/// From the modelled `preferredMaintenanceWindow` documentation: "The default is
/// a 30-minute window selected at random from an 8-hour block of time for each
/// AWS Region, occurring on a random day of the week." Both the day and the slot
/// are DERIVED from the database's own name, so every later read reports the same
/// window; the slot is taken from the second 8-hour block of the day and then
/// walks forward in 30-minute steps until it clears `backup`, because the two
/// windows may not conflict. `None` when the backup window leaves no slot free at
/// all.
pub fn default_preferred_maintenance_window(
    resource_name: &str,
    backup: WindowSpan,
) -> Option<String> {
    let digest = Sha256::digest(resource_name.as_bytes());
    let day = i64::from(digest[2]) % 7;
    let slot = WINDOW_SLOTS_PER_BLOCK + i64::from(digest[3]) % WINDOW_SLOTS_PER_BLOCK;
    (0..WINDOW_SLOTS_PER_DAY).find_map(|step| {
        let start = (slot + step).rem_euclid(WINDOW_SLOTS_PER_DAY) * MIN_WINDOW_MINUTES;
        let candidate = WindowSpan {
            start,
            span: MIN_WINDOW_MINUTES,
        };
        let clear = !windows_conflict(backup, candidate);
        clear.then(|| {
            let day = MAINTENANCE_WINDOW_DAYS[usize::try_from(day).unwrap_or(0)];
            // A 30-minute window never crosses midnight from this block, so both
            // ends name the same day.
            format!(
                "{day}:{}-{day}:{}",
                render_time(start),
                render_time(start + MIN_WINDOW_MINUTES)
            )
        })
    })
}

/// The three versions of a managed relational database's master user password
/// `GetRelationalDatabaseMasterUserPassword$passwordVersion` can name, from the
/// modelled `RelationalDatabasePasswordVersion` enum.
///
/// `CURRENT` is the password the master user is reached with now, `PREVIOUS` the
/// one it replaced, and `PENDING` the one a modification will rotate to
/// `CURRENT`.
pub const RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT: &str = "CURRENT";
/// See [`RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT`].
pub const RELATIONAL_DATABASE_PASSWORD_VERSION_PREVIOUS: &str = "PREVIOUS";
/// See [`RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT`].
pub const RELATIONAL_DATABASE_PASSWORD_VERSION_PENDING: &str = "PENDING";

/// The password version a request that names none asks for: "Default: CURRENT"
/// (the modelled `passwordVersion` documentation).
pub const DEFAULT_RELATIONAL_DATABASE_PASSWORD_VERSION: &str =
    RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT;

/// ONE version of a managed relational database's master user password: the
/// secret itself and the instant that version was created.
///
/// A version is created by the call that sets the password — the create or the
/// `UpdateRelationalDatabase` that named or minted it — and keeps that instant
/// for as long as it is held, so the instant
/// `GetRelationalDatabaseMasterUserPassword$createdAt` reports for a version
/// does not move when the version rotates from `PENDING` to `CURRENT` or from
/// `CURRENT` to `PREVIOUS`.
#[derive(Clone)]
pub struct StoredMasterUserPassword {
    /// The secret. Reported by `GetRelationalDatabaseMasterUserPassword` alone;
    /// every other read of a database masks or omits it.
    pub password: String,
    /// Epoch milliseconds at which this version was created.
    pub created_at_ms: i64,
}

impl StoredMasterUserPassword {
    /// The version `password`, created at `created_at_ms`.
    pub fn new(password: impl Into<String>, created_at_ms: i64) -> Self {
        Self {
            password: password.into(),
            created_at_ms,
        }
    }
}

/// The stored database this version hangs off derives `Debug`, so a formatter
/// reaches every field of it; this one writes the instant and withholds the
/// secret, which is handed out by the one operation that answers with it and
/// nowhere else.
impl std::fmt::Debug for StoredMasterUserPassword {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredMasterUserPassword")
            .field("created_at_ms", &self.created_at_ms)
            .finish_non_exhaustive()
    }
}

/// The members of a database one `UpdateRelationalDatabase` call changes that
/// can be left WAITING: the master user password, the engine version (which a
/// major-version blueprint update moves) and the backup retention setting.
///
/// Exactly the three the modelled `PendingModifiedRelationalDatabaseValues`
/// carries — the structure `RelationalDatabase$pendingModifiedValues` reports a
/// pending modification through — so a change to any OTHER member has no way to
/// be reported pending and therefore takes effect at once.
///
/// An entry is `None` when the call did not ask for that member to change.
#[derive(Debug, Clone, Default)]
pub struct RelationalDatabaseModification {
    /// The password version the master user gets, whether the caller supplied
    /// the secret or asked the service to mint one. Created at the instant of
    /// the call that asked for it, which is the instant it keeps once it reaches
    /// the database.
    pub master_user_password: Option<StoredMasterUserPassword>,
    /// The blueprint the database moves to: a later major version of the engine
    /// it already runs, which is what `engineVersion` then reports.
    pub blueprint: Option<RelationalDatabaseBlueprintSpec>,
    /// Whether automated backup retention is on.
    pub backup_retention_enabled: Option<bool>,
}

impl RelationalDatabaseModification {
    /// Whether this call asked for none of the three to change, so there is
    /// nothing to apply and nothing to leave pending.
    pub fn is_empty(&self) -> bool {
        self.master_user_password.is_none()
            && self.blueprint.is_none()
            && self.backup_retention_enabled.is_none()
    }

    /// Take `later`'s entries over this one's: a second call that modifies the
    /// same member while the first is still waiting replaces what it will be
    /// applied with, and leaves the members it says nothing about pending.
    pub(super) fn merge(&mut self, later: Self) {
        if let Some(password) = later.master_user_password {
            self.master_user_password = Some(password);
        }
        if let Some(blueprint) = later.blueprint {
            self.blueprint = Some(blueprint);
        }
        if let Some(enabled) = later.backup_retention_enabled {
            self.backup_retention_enabled = Some(enabled);
        }
    }
}

/// A modification an `UpdateRelationalDatabase` call left for the database's
/// next maintenance window, and the instant the clock applies it at.
#[derive(Debug, Clone)]
pub struct PendingRelationalDatabaseModification {
    /// Epoch milliseconds at which the modification becomes due.
    ///
    /// AWS applies a deferred modification in the database's own
    /// `preferredMaintenanceWindow`, which is up to a week away and no caller
    /// can be made to wait for here, so the instant is the one every other
    /// clock-owed change of this service uses
    /// ([`STATE_TRANSITION_MILLIS`] after the call). The span is deliberately
    /// NON-ZERO, so a caller that reads straight after the call still observes
    /// the modification pending exactly as it does against AWS.
    pub at_ms: i64,
    /// What becomes of the database when it is due.
    pub change: RelationalDatabaseModification,
}

impl PendingRelationalDatabaseModification {
    /// This modification as the modelled
    /// `PendingModifiedRelationalDatabaseValues`.
    ///
    /// A member nothing is waiting on is left out rather than reported null, and
    /// a waiting password is reported MASKED
    /// ([`MASKED_MASTER_USER_PASSWORD`]).
    pub fn view(&self) -> crate::ops::types::PendingModifiedRelationalDatabaseValues {
        crate::ops::types::PendingModifiedRelationalDatabaseValues {
            master_user_password: self
                .change
                .master_user_password
                .as_ref()
                .map(|_| MASKED_MASTER_USER_PASSWORD.to_string()),
            engine_version: self
                .change
                .blueprint
                .map(|blueprint| blueprint.engine_version.to_string()),
            backup_retention_enabled: self.change.backup_retention_enabled,
        }
    }
}

/// One entry of a managed relational database's event log: what the service did
/// to the database, when it did it, and the categories that change falls in —
/// the modelled `RelationalDatabaseEvent` as this store holds it.
///
/// The log is the DATABASE's own: it is appended to by the service's own changes
/// to that database and by nothing else, so a read of it reports what happened
/// rather than anything the reading request carried.
#[derive(Debug, Clone)]
pub struct StoredRelationalDatabaseEvent {
    /// This entry's position in its database's log, drawn from the entry before
    /// it so it is never reused: the `pageToken`
    /// `GetRelationalDatabaseEvents` names a page by, which an entry needs
    /// because nothing else about an event identifies it — two changes can carry
    /// the same message at the same instant.
    pub sequence: u64,
    /// Epoch milliseconds at which the change happened.
    pub created_at_ms: i64,
    /// What happened, from this module's `RELATIONAL_DATABASE_EVENT_*` wording.
    pub message: String,
    /// The categories the change falls in, from the
    /// `RELATIONAL_DATABASE_EVENT_CATEGORY_*` values.
    pub categories: Vec<&'static str>,
}

impl StoredRelationalDatabaseEvent {
    /// This entry as the modelled `RelationalDatabaseEvent` of the database
    /// `resource` names — "the database that the database event relates to".
    pub fn view(&self, resource: &str) -> crate::ops::types::RelationalDatabaseEvent {
        crate::ops::types::RelationalDatabaseEvent {
            resource: Some(resource.to_string()),
            created_at: Some(timestamp(self.created_at_ms)),
            message: Some(self.message.clone()),
            // A list member is reported as a list, so a typed caller iterating
            // the categories of an event is handed one.
            event_categories: Some(
                self.categories
                    .iter()
                    .map(|category| (*category).to_string())
                    .collect(),
            ),
        }
    }

    /// The page token that names THIS entry.
    pub fn page_token(&self) -> String {
        self.sequence.to_string()
    }

    /// This entry as one LINE of the database's log file — the modelled
    /// `LogEvent` `GetRelationalDatabaseLogEvents` reports: the instant the line
    /// was written and what it says.
    ///
    /// The same happening the event log reports, seen through the other API:
    /// `GetRelationalDatabaseEvents` reports it as the service's event (with the
    /// database it relates to and the categories it falls in), and a log stream
    /// reports it as the line the engine wrote, which carries neither.
    pub fn log_event_view(&self) -> crate::ops::types::LogEvent {
        crate::ops::types::LogEvent {
            created_at: Some(timestamp(self.created_at_ms)),
            message: Some(self.message.clone()),
        }
    }
}

/// Which end of a log stream a `GetRelationalDatabaseLogEvents` page is read
/// from, and the position in that stream it is read at.
///
/// A position is a line's [`StoredRelationalDatabaseEvent::sequence`], so a
/// token NAMES a place in the log rather than carrying an index into one answer:
/// a stream that has been written to since the token was issued still answers
/// the lines after the one the token stopped at, and a line retention has
/// dropped cannot make the walk skip or repeat another.
///
/// Rendered as the direction (`f` forward, `b` backward) and that position —
/// the two tokens this operation reports are values a caller only ever hands
/// back, so the rendering is this store's own and is parsed back by
/// [`Self::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogEventPageToken {
    /// Read the line AT this position and the ones after it (forward), or the
    /// ones BEFORE it (backward).
    pub forward: bool,
    /// The position in the stream the next read starts (or ends) at.
    pub position: u64,
}

impl LogEventPageToken {
    /// The forward token at `position`.
    pub fn forward(position: u64) -> Self {
        Self {
            forward: true,
            position,
        }
    }

    /// The backward token at `position`.
    pub fn backward(position: u64) -> Self {
        Self {
            forward: false,
            position,
        }
    }

    /// `token` as this store issues one, or `None` when it is not one at all —
    /// which the caller turns into a refusal rather than quietly reading from
    /// the head of the log.
    ///
    /// TOTAL on every input: the value is split on the one separator and the
    /// position is parsed, so an arbitrary caller-supplied string answers
    /// `None` instead of panicking.
    pub fn parse(token: &str) -> Option<Self> {
        let (direction, position) = token.split_once('/')?;
        let forward = match direction {
            "f" => true,
            "b" => false,
            _ => return None,
        };
        Some(Self {
            forward,
            position: position.parse().ok()?,
        })
    }
}

impl std::fmt::Display for LogEventPageToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let direction = if self.forward { 'f' } else { 'b' };
        write!(formatter, "{direction}/{}", self.position)
    }
}

/// One page of a log stream: the lines it reported, and the two tokens that read
/// the page after it and the page before it.
///
/// Both tokens are always present, whether the page carried a line or not: a
/// stream is appended to while a caller reads it, so a caller that has reached
/// the end of one still needs the token that picks up the lines written next.
#[derive(Debug, Clone)]
pub struct LogEventPage {
    /// The lines of the page, in the order they were written.
    pub events: Vec<crate::ops::types::LogEvent>,
    /// `nextForwardToken`: the lines after this page.
    pub next_forward_token: LogEventPageToken,
    /// `nextBackwardToken`: the lines before this page.
    pub next_backward_token: LogEventPageToken,
}

/// One stored managed relational database: every member `GetRelationalDatabase`
/// reports, the master user password `GetRelationalDatabaseMasterUserPassword`
/// answers with, and the clock-owed transition its state is waiting on.
#[derive(Debug, Clone)]
pub struct StoredRelationalDatabase {
    /// The database name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The blueprint the request named, resolved against the published catalog:
    /// the engine and the version every read reports come from it.
    pub blueprint: RelationalDatabaseBlueprintSpec,
    /// The bundle the request named, resolved against the published catalog: the
    /// hardware every read reports comes from it.
    pub bundle: RelationalDatabaseBundleSpec,
    pub master_database_name: String,
    pub master_username: String,
    /// The CURRENT version of the master user password: the secret the request
    /// carried, the one the service generated for a request that named none, or
    /// the one a later `UpdateRelationalDatabase` set. Kept because
    /// `GetRelationalDatabaseMasterUserPassword` is what answers with it; no
    /// member of `RelationalDatabase` exposes it. Every database carries one —
    /// it is the credential its engine authenticates the master user by.
    pub master_user_password: StoredMasterUserPassword,
    /// The PREVIOUS version of the master user password: the version the CURRENT
    /// one replaced, which `GetRelationalDatabaseMasterUserPassword` answers for
    /// `passwordVersion` `PREVIOUS`. Absent until a password change has replaced
    /// one.
    pub previous_master_user_password: Option<StoredMasterUserPassword>,
    pub preferred_backup_window: String,
    pub preferred_maintenance_window: String,
    pub publicly_accessible: bool,
    pub backup_retention_enabled: bool,
    /// The standby's Availability Zone, on a high-availability bundle only: "The
    /// secondary database is used for failover support of a high availability
    /// database."
    pub secondary_availability_zone: Option<String>,
    /// The master endpoint address, assigned at creation so every later read
    /// reports the same one. See [`RELATIONAL_DATABASE_ENDPOINT_SUFFIX`].
    pub endpoint_address: String,
    /// The certificate authority this database's server certificate is issued
    /// under, as `caCertificateIdentifier` reports it: the service's default
    /// ([`RELATIONAL_DATABASE_CA_CERTIFICATE_IDENTIFIER`]) until an
    /// `UpdateRelationalDatabase` call associates another published one with the
    /// database.
    pub ca_certificate_identifier: String,
    /// The current state: [`RELATIONAL_DATABASE_STATE_CREATING`],
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`],
    /// [`RELATIONAL_DATABASE_STATE_REBOOTING`],
    /// [`RELATIONAL_DATABASE_STATE_STOPPING`],
    /// [`RELATIONAL_DATABASE_STATE_STOPPED`] or
    /// [`RELATIONAL_DATABASE_STATE_STARTING`].
    pub state: &'static str,
    /// The state the clock owes this database, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The parameters an `UpdateRelationalDatabaseParameters` call has set on
    /// this database, by parameter name — the per-database half of what
    /// `GetRelationalDatabaseParameters` reports, the other half being the
    /// engine's own published set ([`relational_database_parameter_specs`]). A
    /// parameter nothing has set is absent, and reports what the engine and this
    /// database's own hardware give it.
    pub parameter_overrides: BTreeMap<String, StoredParameterOverride>,
    /// The modification an `UpdateRelationalDatabase` call has left for this
    /// database's next maintenance window, while one is outstanding — what
    /// `pendingModifiedValues` reports until the clock applies it.
    pub pending_modification: Option<PendingRelationalDatabaseModification>,
    /// This database's event log, oldest first — the source of truth for what
    /// `GetRelationalDatabaseEvents` reports.
    ///
    /// One entry per change the SERVICE made to this database: its engine going
    /// down and coming back up, a snapshot of it being taken, a modification or
    /// a parameter value reaching it. Appended through
    /// [`StoredRelationalDatabase::record_event`], which is also what keeps the
    /// log bounded.
    pub events: Vec<StoredRelationalDatabaseEvent>,
    /// The token the next entry of this database's log is named by, counted on
    /// from the last one minted rather than from the entries still held, so a
    /// token is never reused after retention has dropped the entry it named.
    pub next_event_sequence: u64,
}

impl StoredRelationalDatabase {
    /// `parameters` as THIS database reports them: an entry an
    /// `UpdateRelationalDatabaseParameters` call has set on this database
    /// carries the value it set and the apply method it was set with, and every
    /// other entry is the engine's published one, unchanged.
    ///
    /// The value an update set is reported straight away, whichever apply method
    /// it used: the modification reaches the database's parameter group when the
    /// call is answered, and whether the RUNNING engine has loaded it is what
    /// [`Self::parameter_apply_status`] reports instead.
    pub fn applied_parameters(
        &self,
        parameters: Vec<RelationalDatabaseParameter>,
    ) -> Vec<RelationalDatabaseParameter> {
        parameters
            .into_iter()
            .map(|parameter| {
                let Some(set) = parameter
                    .parameter_name
                    .as_deref()
                    .and_then(|name| self.parameter_overrides.get(name))
                else {
                    return parameter;
                };
                RelationalDatabaseParameter {
                    parameter_value: Some(set.value.clone()),
                    apply_method: Some(set.apply_method.to_string()),
                    ..parameter
                }
            })
            .collect()
    }

    /// Apply `update` to this database's parameter group.
    ///
    /// The value is carried from here on, and a value applied
    /// `pending-reboot` also leaves the RUNNING engine behind — which is what
    /// [`Self::parameter_apply_status`] reports.
    ///
    /// The modification reaches the parameter group when the call is answered,
    /// so that is a change to the database and its log carries one entry for it.
    ///
    /// ONE rule for the whole log: an entry says something about this database
    /// CHANGED, so the apply is logged only where it moves the parameter group —
    /// a value, or the apply method it was set with, that differs from the one
    /// this database already carried. Re-sending the value a parameter already
    /// holds changes nothing and logs nothing, exactly as re-sending a
    /// modification does ([`Self::apply_modification`]).
    pub fn set_parameter(&mut self, update: RelationalDatabaseParameterUpdate, now_ms: i64) {
        let pending_reboot = update.apply_method == PARAMETER_APPLY_METHOD_PENDING_REBOOT;
        let parameter_name = update.spec.parameter_name.to_string();
        let moved = self
            .parameter_overrides
            .get(&parameter_name)
            .is_none_or(|held| {
                held.value != update.value || held.apply_method != update.apply_method
            });
        self.parameter_overrides.insert(
            parameter_name.clone(),
            StoredParameterOverride {
                value: update.value,
                apply_method: update.apply_method,
                pending_reboot,
            },
        );
        if moved {
            self.record_event(
                now_ms,
                format!("{RELATIONAL_DATABASE_EVENT_PARAMETER_GROUP_CHANGED}: {parameter_name}"),
                &[RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
            );
        }
    }

    /// Append one entry to this database's event log: `message` happened to it
    /// at `at_ms`, in `categories`.
    ///
    /// The ONE place an event is logged, so every change the service makes to a
    /// database is recorded the same way, and the log is bounded here rather
    /// than by whoever logs: an entry older than the longest window a caller may
    /// ask for ([`RELATIONAL_DATABASE_EVENT_RETENTION_MILLIS`]) can never be
    /// reported again, and the count is capped at
    /// [`MAX_RELATIONAL_DATABASE_EVENTS`] for the same reason the operation log
    /// is capped.
    pub fn record_event(
        &mut self,
        at_ms: i64,
        message: impl Into<String>,
        categories: &[&'static str],
    ) {
        // The token is drawn from a counter of its own rather than from the
        // entry before it, so a number is never handed out twice however many
        // entries retention has dropped: a token a client is still holding can
        // then only name the entry it was reported for.
        let sequence = self.next_event_sequence;
        self.next_event_sequence = self.next_event_sequence.wrapping_add(1);
        let event = StoredRelationalDatabaseEvent {
            sequence,
            created_at_ms: at_ms,
            message: message.into(),
            categories: categories.to_vec(),
        };
        // The log is held OLDEST FIRST, which is the order it is reported in, so
        // an entry is placed by its instant rather than appended: two changes
        // that fall due in the same pass of the clock are not necessarily made
        // due in the order the pass walks them. An entry at the same instant as
        // one already held goes after it, so changes that share an instant keep
        // the order they happened in.
        let at = self
            .events
            .iter()
            .rposition(|held| held.created_at_ms <= at_ms)
            .map_or(0, |position| position.saturating_add(1));
        self.events.insert(at, event);
        let oldest_reportable = at_ms.saturating_sub(RELATIONAL_DATABASE_EVENT_RETENTION_MILLIS);
        self.events
            .retain(|event| event.created_at_ms >= oldest_reportable);
        if let Some(excess) = self.events.len().checked_sub(MAX_RELATIONAL_DATABASE_EVENTS) {
            self.events.drain(..excess);
        }
    }

    /// One page of this database's events inside the window that opens at
    /// `since_ms`, and the token naming the first event of the page after it —
    /// present only while events remain.
    ///
    /// Oldest first, as the log holds them, and at most
    /// [`RELATIONAL_DATABASE_EVENT_PAGE_SIZE`] of them. The walk is
    /// [`catalog_page`], shared with this service's other paged listings; only
    /// the token an entry is named by is this log's own
    /// ([`StoredRelationalDatabaseEvent::page_token`]). `None` is the answer
    /// when `page_token` names no event of that window — there is no page to
    /// report, which the caller turns into a refusal — and an absent token asks
    /// for the first page.
    pub fn event_page(
        &self,
        since_ms: i64,
        page_token: Option<&str>,
    ) -> Option<(
        Vec<crate::ops::types::RelationalDatabaseEvent>,
        Option<String>,
    )> {
        let reported = self
            .events
            .iter()
            .filter(|event| event.created_at_ms >= since_ms)
            .collect::<Vec<_>>();
        let (page, next_page_token) = catalog_page(
            &reported,
            RELATIONAL_DATABASE_EVENT_PAGE_SIZE,
            page_token,
            // Owned, because an event is named by its sequence number rather
            // than by a string member it holds. The walk is over at most
            // [`MAX_RELATIONAL_DATABASE_EVENTS`] entries, so it is bounded.
            |event| Some(std::borrow::Cow::Owned(event.page_token())),
        )?;
        Some((
            page.iter().map(|event| event.view(&self.name)).collect(),
            next_page_token,
        ))
    }

    /// Change the three members of this database a modification can carry,
    /// either now (`immediately`) or at its next maintenance window.
    ///
    /// "When true, applies changes immediately. When false, applies changes
    /// during the preferred maintenance window" (the modelled
    /// `applyImmediately` documentation), which is why a deferred change is held
    /// on the record and reported by `pendingModifiedValues` rather than written
    /// into it. Applying immediately applies what an EARLIER call left waiting
    /// too — the member it set is a change the database is already carrying
    /// towards, and nothing may be left waiting once a call has said to apply
    /// now — with this call's own value winning where both name one. Answers the
    /// instant the change takes effect at, or `None` when it already has —
    /// which is also what a call that changes none of the three answers.
    pub fn modify(
        &mut self,
        change: RelationalDatabaseModification,
        immediately: bool,
        now_ms: i64,
    ) -> Option<i64> {
        if immediately {
            let mut merged = self
                .pending_modification
                .take()
                .map(|pending| pending.change)
                .unwrap_or_default();
            merged.merge(change);
            if !merged.is_empty() {
                self.apply_modification(merged, now_ms);
            }
            return None;
        }
        if change.is_empty() {
            return None;
        }
        let at_ms = now_ms.saturating_add(STATE_TRANSITION_MILLIS);
        match &mut self.pending_modification {
            Some(pending) => {
                pending.at_ms = at_ms;
                pending.change.merge(change);
            }
            None => {
                self.pending_modification = Some(PendingRelationalDatabaseModification {
                    at_ms,
                    change,
                })
            }
        }
        Some(at_ms)
    }

    /// Write `change` into this record: the database carries the new values from
    /// here on, and nothing is waiting on them any more.
    ///
    /// `at_ms` is the instant the modification reached the database — the call's
    /// own instant when it asked to apply now, and the instant it fell due when
    /// the clock applied it — which is what the entry this logs is timestamped
    /// at.
    pub(super) fn apply_modification(&mut self, change: RelationalDatabaseModification, at_ms: i64) {
        // A member is a CHANGE only where the value differs from the one the
        // database already carries: a modification that asks for what the
        // database already has leaves it as it is and logs nothing, the same
        // rule the members an update applies at once are held to.
        let mut changed: Vec<&'static str> = Vec::new();
        if let Some(password) = change.master_user_password {
            // A modification that asks for the password the database already
            // holds replaces nothing: the version it holds keeps both its
            // secret and the instant it was created at, so a read of CURRENT
            // does not move.
            if self.master_user_password.password != password.password {
                changed.push("masterUserPassword");
                // The version the new one replaces becomes PREVIOUS, keeping
                // the instant it was created at: it is the same version, one
                // step further back.
                self.previous_master_user_password =
                    Some(std::mem::replace(&mut self.master_user_password, password));
            }
        }
        if let Some(blueprint) = change.blueprint {
            if blueprint.blueprint_id() != self.blueprint.blueprint_id() {
                changed.push("engineVersion");
            }
            self.blueprint = blueprint;
        }
        if let Some(enabled) = change.backup_retention_enabled {
            if enabled != self.backup_retention_enabled {
                changed.push("backupRetentionEnabled");
            }
            self.backup_retention_enabled = enabled;
        }
        if !changed.is_empty() {
            self.record_event(
                at_ms,
                format!(
                    "{RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED}: {}",
                    changed.join(", ")
                ),
                &[RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE],
            );
        }
    }

    /// The version of this database's master user password `version` names —
    /// one of the `RELATIONAL_DATABASE_PASSWORD_VERSION_*` values — or `None`
    /// when this database holds no such version.
    ///
    /// `CURRENT` is the password the master user is reached with now, `PREVIOUS`
    /// the version it replaced, and `PENDING` the version a modification
    /// waiting for this database's maintenance window will rotate to `CURRENT`:
    /// "After the PENDING password rotates to CURRENT, the PENDING password is
    /// no longer available" (the modelled `passwordVersion` documentation),
    /// which holds here because the version is taken from the modification
    /// still outstanding, and the clock takes that away when it applies it.
    pub fn master_user_password_version(
        &self,
        version: &str,
    ) -> Option<&StoredMasterUserPassword> {
        match version {
            RELATIONAL_DATABASE_PASSWORD_VERSION_CURRENT => Some(&self.master_user_password),
            RELATIONAL_DATABASE_PASSWORD_VERSION_PREVIOUS => {
                self.previous_master_user_password.as_ref()
            }
            RELATIONAL_DATABASE_PASSWORD_VERSION_PENDING => self
                .pending_modification
                .as_ref()
                .and_then(|pending| pending.change.master_user_password.as_ref()),
            _ => None,
        }
    }

    /// Apply the modification the clock owes this database, once its maintenance
    /// window is due. A no-op while nothing is waiting, or while the instant is
    /// still ahead.
    pub fn apply_due_modification(&mut self, now_ms: i64) {
        if self
            .pending_modification
            .as_ref()
            .is_none_or(|pending| pending.at_ms > now_ms)
        {
            return;
        }
        if let Some(pending) = self.pending_modification.take() {
            self.apply_modification(pending.change, pending.at_ms);
        }
    }

    /// Start the restart of this database's engine: the database reports
    /// [`RELATIONAL_DATABASE_STATE_REBOOTING`] and the clock owes it
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`] again.
    ///
    /// A restart happens INSIDE the database: nothing else on the record is
    /// rewritten — the ARN, the hardware, the credentials, the windows and the
    /// master endpoint a client connects to are all the ones the database
    /// already had, exactly as an RDS DB instance keeps its endpoint across a
    /// reboot.
    ///
    /// PRECONDITION: the database has no transition outstanding. A reboot is
    /// accepted only in [`RELATIONAL_DATABASE_STATE_AVAILABLE`], the one state
    /// where nothing is owed, so the `due` written here replaces none — a caller
    /// that reached this from a state with a transition pending would drop it.
    pub fn begin_reboot(&mut self, now_ms: i64) {
        self.state = RELATIONAL_DATABASE_STATE_REBOOTING;
        self.due = Some(DueNamedState::after(
            now_ms,
            RELATIONAL_DATABASE_STATE_AVAILABLE,
        ));
        // The engine goes down here and comes back up when the transition is
        // due, so the restart is TWO entries in the log, each at its own
        // instant.
        self.record_event(
            now_ms,
            RELATIONAL_DATABASE_EVENT_SHUTDOWN,
            &[RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        );
    }

    /// Start shutting this database's engine down: the database reports
    /// [`RELATIONAL_DATABASE_STATE_STOPPING`] and the clock owes it
    /// [`RELATIONAL_DATABASE_STATE_STOPPED`].
    ///
    /// Stopping happens INSIDE the database, exactly as a restart does: nothing
    /// else on the record is rewritten, the master endpoint included — an RDS DB
    /// instance keeps its endpoint while it is stopped, which is what lets the
    /// same address serve again once it is started.
    ///
    /// PRECONDITION: the database has no transition outstanding. A stop is
    /// accepted only in [`RELATIONAL_DATABASE_STATE_AVAILABLE`], the one state
    /// where nothing is owed, so the `due` written here replaces none.
    pub fn begin_stop(&mut self, now_ms: i64) {
        self.state = RELATIONAL_DATABASE_STATE_STOPPING;
        self.due = Some(DueNamedState::after(
            now_ms,
            RELATIONAL_DATABASE_STATE_STOPPED,
        ));
        // As with a restart: the engine goes down here, and the log carries a
        // second entry when it has finished shutting down.
        self.record_event(
            now_ms,
            RELATIONAL_DATABASE_EVENT_SHUTDOWN,
            &[RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY],
        );
    }

    /// Start this database's engine back up: the database reports
    /// [`RELATIONAL_DATABASE_STATE_STARTING`] and the clock owes it
    /// [`RELATIONAL_DATABASE_STATE_AVAILABLE`].
    ///
    /// Starting re-provisions nothing — the record is the one the database had
    /// while it was stopped, the master endpoint a client connects to above all.
    ///
    /// The `due` written here REPLACES the one a stopped database carries: the
    /// service's own seven-day automatic start
    /// ([`RELATIONAL_DATABASE_AUTO_START_AFTER_MILLIS`]) is what this call does
    /// by hand, so it is no longer owed once the caller has asked for it.
    pub fn begin_start(&mut self, now_ms: i64) {
        self.state = RELATIONAL_DATABASE_STATE_STARTING;
        self.due = Some(DueNamedState::after(
            now_ms,
            RELATIONAL_DATABASE_STATE_AVAILABLE,
        ));
    }

    /// The engine has (re)started — after a `RebootRelationalDatabase` call, or
    /// after a `StartRelationalDatabase` brought a stopped database back up — so
    /// every parameter value that was waiting for a restart is now loaded by it:
    /// "Parameters marked with a pending-reboot apply type are applied only
    /// after the database is rebooted using the reboot relational database
    /// operation" (the modelled `UpdateRelationalDatabaseParameters`
    /// documentation).
    ///
    /// The VALUES are untouched — the parameter group carried them already; what
    /// changes is that the running engine has them, which is what
    /// [`Self::parameter_apply_status`] reports.
    pub fn load_pending_parameters(&mut self) {
        for set in self.parameter_overrides.values_mut() {
            set.pending_reboot = false;
        }
    }

    /// `RelationalDatabase$parameterApplyStatus`: DERIVED from the parameter
    /// group rather than tracked beside it, where the two could disagree — a
    /// database is [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`] exactly while it
    /// holds a parameter value the running engine has not loaded, and
    /// [`PARAMETER_APPLY_STATUS_IN_SYNC`] otherwise.
    pub fn parameter_apply_status(&self) -> &'static str {
        if self
            .parameter_overrides
            .values()
            .any(|set| set.pending_reboot)
        {
            PARAMETER_APPLY_STATUS_PENDING_REBOOT
        } else {
            PARAMETER_APPLY_STATUS_IN_SYNC
        }
    }

    /// This record as the modelled `RelationalDatabase`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabase {
        crate::ops::types::RelationalDatabase {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_RELATIONAL_DATABASE.to_string()),
            tags: Some(self.tags.clone()),
            relational_database_blueprint_id: Some(self.blueprint.blueprint_id()),
            relational_database_bundle_id: Some(self.bundle.bundle_id.to_string()),
            master_database_name: Some(self.master_database_name.clone()),
            hardware: Some(crate::ops::types::RelationalDatabaseHardware {
                cpu_count: Some(self.bundle.cpu_count),
                disk_size_in_gb: Some(self.bundle.disk_size_in_gb),
                ram_size_in_gb: Some(self.bundle.ram_size_in_gb),
            }),
            state: Some(self.state.to_string()),
            secondary_availability_zone: self.secondary_availability_zone.clone(),
            backup_retention_enabled: Some(self.backup_retention_enabled),
            // What an `UpdateRelationalDatabase` call left for this database's
            // next maintenance window, while something is waiting; a database
            // with nothing mid-modification reports nothing rather than an
            // empty structure.
            pending_modified_values: self
                .pending_modification
                .as_ref()
                .map(PendingRelationalDatabaseModification::view),
            engine: Some(self.blueprint.engine.to_string()),
            engine_version: Some(self.blueprint.engine_version.to_string()),
            // "The latest point in time to which the database can be restored":
            // no automated backup has been taken yet — the first one is taken in
            // the database's own backup window — so there is no such point.
            latest_restorable_time: None,
            master_username: Some(self.master_username.clone()),
            parameter_apply_status: Some(self.parameter_apply_status().to_string()),
            preferred_backup_window: Some(self.preferred_backup_window.clone()),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            publicly_accessible: Some(self.publicly_accessible),
            // The endpoint is announced once the database has finished being
            // provisioned: a database still being created accepts no connection,
            // and reports no address to make one to. Once announced it is the
            // database's from then on — a restart does not take it away, exactly
            // as an RDS DB instance keeps its endpoint across a reboot — so it is
            // reported for every state other than `creating`.
            master_endpoint: (self.state != RELATIONAL_DATABASE_STATE_CREATING).then(|| {
                crate::ops::types::RelationalDatabaseEndpoint {
                    port: Some(relational_database_port(self.blueprint.engine)),
                    address: Some(self.endpoint_address.clone()),
                }
            }),
            // A list member is reported EMPTY rather than left out: no
            // maintenance action is pending on a database this slice holds, and
            // an absent member hands a typed caller a null where it iterates.
            pending_maintenance_actions: Some(Vec::new()),
            ca_certificate_identifier: Some(self.ca_certificate_identifier.clone()),
        }
    }

    /// The log streams this database publishes right now, as
    /// `GetRelationalDatabaseLogStreams` answers them.
    ///
    /// A log stream is a log file the database's ENGINE writes, so the set is the
    /// engine's own ([`relational_database_log_streams`]) — and it exists only
    /// once that engine has run: a database still in
    /// [`RELATIONAL_DATABASE_STATE_CREATING`] has started no engine and written
    /// no log, so it publishes no stream, exactly as it announces no master
    /// endpoint. Every later state keeps the files — a database in
    /// [`RELATIONAL_DATABASE_STATE_REBOOTING`] has already written its error log
    /// and the restart is itself logged — so the gate is on `creating` alone,
    /// the same one `master_endpoint` uses.
    pub fn log_streams(&self) -> Vec<String> {
        if self.state == RELATIONAL_DATABASE_STATE_CREATING {
            return Vec::new();
        }
        relational_database_log_streams(self.blueprint.engine)
            .iter()
            .map(|stream| (*stream).to_string())
            .collect()
    }

    /// Whether `stream` is one of the log streams this database publishes right
    /// now — the set [`Self::log_streams`] answers, read without building it.
    pub fn publishes_log_stream(&self, stream: &str) -> bool {
        self.state != RELATIONAL_DATABASE_STATE_CREATING
            && relational_database_log_streams(self.blueprint.engine).contains(&stream)
    }

    /// The lines the log stream `stream` of this database holds, in the order
    /// they were written.
    ///
    /// A log stream is a log FILE the database's engine writes, so its lines are
    /// what that engine has done — and what this store holds of that is the
    /// database's own event log: the entries in the
    /// [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`] and
    /// [`RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY`] categories are the
    /// engine coming up and going down, which is exactly what an engine records
    /// in its ERROR log. So an error-log stream
    /// ([`RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX`]) reports those entries
    /// rather than this record keeping a second copy of them, and the two reads
    /// of one happening — the service's event and the engine's log line — can
    /// never disagree.
    ///
    /// A QUERY log (`general/`, `slowquery/`) holds a line per statement a
    /// CLIENT ran, and this emulator serves no database protocol, so no
    /// statement has ever reached an engine here and every one of those streams
    /// is empty — the same reason a container service's log reports no line
    /// (`crate::ops::get_container_log`).
    ///
    /// Ordered by sequence, which is the order the service logged them in: a
    /// file is read in the order it was WRITTEN, while the event log's own
    /// reader orders by instant.
    pub(super) fn log_stream_lines(&self, stream: &str) -> Vec<&StoredRelationalDatabaseEvent> {
        if !stream.starts_with(RELATIONAL_DATABASE_ERROR_LOG_STREAM_PREFIX) {
            return Vec::new();
        }
        let mut lines = self
            .events
            .iter()
            .filter(|event| {
                event
                    .categories
                    .contains(&RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION)
                    || event
                        .categories
                        .contains(&RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY)
            })
            .collect::<Vec<&StoredRelationalDatabaseEvent>>();
        lines.sort_by_key(|line| line.sequence);
        lines
    }

    /// One page of the log stream `stream` of this database: the lines it holds
    /// inside the half-open interval `[from_ms, to_ms)` — an absent bound leaves
    /// that side of the interval open — read from the end `start_from_head`
    /// names, or from the place `page_token` stopped at when the request carried
    /// one.
    ///
    /// At most [`RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE`] lines, always in the
    /// order they were written whichever end the read started from: the end
    /// chooses WHICH lines the first page carries, not the order they are
    /// reported in.
    ///
    /// A stream this database does not publish answers an empty page here; the
    /// caller refuses such a name ([`Self::publishes_log_stream`]) rather than
    /// reporting one.
    pub fn log_event_page(
        &self,
        stream: &str,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        start_from_head: bool,
        page_token: Option<LogEventPageToken>,
    ) -> LogEventPage {
        // A page carries at least one line, so a walk over a non-empty stream
        // always advances.
        let page_size = RELATIONAL_DATABASE_LOG_EVENT_PAGE_SIZE.max(1);
        // "Specified in Coordinated Universal Time (UTC)": the interval is
        // half-open, so a line written AT `startTime` is inside it and one
        // written at `endTime` is not — which is what lets a caller walk a log
        // in abutting intervals without reading a line twice.
        let reported = self
            .log_stream_lines(stream)
            .into_iter()
            .filter(|line| from_ms.is_none_or(|from_ms| line.created_at_ms >= from_ms))
            .filter(|line| to_ms.is_none_or(|to_ms| line.created_at_ms < to_ms))
            .collect::<Vec<&StoredRelationalDatabaseEvent>>();
        let page: Vec<&StoredRelationalDatabaseEvent> = match page_token {
            // Forward from the position the last page stopped at: the line at
            // that position has not been reported yet.
            Some(token) if token.forward => reported
                .iter()
                .copied()
                .filter(|line| line.sequence >= token.position)
                .take(page_size)
                .collect(),
            // Backward from it: the page that ENDS just before that line.
            Some(token) => {
                let before = reported
                    .iter()
                    .copied()
                    .filter(|line| line.sequence < token.position)
                    .collect::<Vec<&StoredRelationalDatabaseEvent>>();
                let start = before.len().saturating_sub(page_size);
                before.into_iter().skip(start).collect()
            }
            // "If true is specified, the log event starts from the head of the
            // log. If false is specified, the log event starts from the tail of
            // the log."
            None if start_from_head => reported.iter().copied().take(page_size).collect(),
            None => {
                let start = reported.len().saturating_sub(page_size);
                reported.iter().copied().skip(start).collect()
            }
        };
        let (forward_at, backward_at) = match (page.first(), page.last()) {
            // The page after this one starts at the line after its last, and
            // the page before it ends at its first.
            (Some(first), Some(last)) => (last.sequence.saturating_add(1), first.sequence),
            // Nothing was reported, so the read stopped where it started: at
            // the token's own position, at the head of the log (position zero)
            // for a read from the head, and at its end — the position the next
            // line written will carry — for a read from the tail. A caller that
            // has reached the end of the stream can poll the forward token for
            // the lines written after this answer.
            _ => {
                let stopped = match page_token {
                    Some(token) => token.position,
                    None if start_from_head => 0,
                    None => self.next_event_sequence,
                };
                (stopped, stopped)
            }
        };
        LogEventPage {
            events: page
                .iter()
                .map(|line| line.log_event_view())
                .collect::<Vec<crate::ops::types::LogEvent>>(),
            next_forward_token: LogEventPageToken::forward(forward_at),
            next_backward_token: LogEventPageToken::backward(backward_at),
        }
    }
}

/// One stored managed relational database snapshot: every member
/// `GetRelationalDatabaseSnapshot` and `GetRelationalDatabaseSnapshots` report,
/// plus the clock-owed transition its state is waiting on.
///
/// A snapshot is a point-in-time copy, so everything it says about its SOURCE is
/// copied out of the source record at the instant the snapshot was taken and
/// never followed afterwards: the snapshot keeps reporting the engine, version,
/// disk size, bundle and blueprint the database had then, even after the
/// database is resized or deleted.
#[derive(Debug, Clone)]
pub struct StoredRelationalDatabaseSnapshot {
    /// The snapshot name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The source database's engine, copied from its blueprint.
    pub engine: String,
    /// The source database's engine version, copied from its blueprint.
    pub engine_version: String,
    /// The size of the snapshotted database disk in GB, copied from the source
    /// database's bundle.
    pub size_in_gb: i32,
    /// The name of the database the source database served, copied from the
    /// source record: a snapshot captures the database it was taken of, so
    /// `CreateRelationalDatabaseFromSnapshot` restores a database serving the
    /// same database name — even after the source has been deleted. No member of
    /// `RelationalDatabaseSnapshot` exposes it.
    pub master_database_name: String,
    /// The master user name the source database was created with, copied from
    /// the source record for the same reason: a restored database is reached
    /// with the credentials of the database the snapshot was taken of.
    pub master_username: String,
    /// The master user password the source database held, copied from the source
    /// record so a restore reports the credentials the snapshot captured.
    /// Absent when the source carried none.
    pub master_user_password: Option<String>,
    /// The current state: [`RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING`] or
    /// [`RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE`].
    pub state: &'static str,
    /// The state the clock owes this snapshot, while one is outstanding.
    pub due: Option<DueNamedState>,
    /// The name of the database the snapshot was taken of.
    pub from_relational_database_name: String,
    /// That database's ARN.
    pub from_relational_database_arn: String,
    /// That database's bundle id.
    pub from_relational_database_bundle_id: String,
    /// That database's blueprint id.
    pub from_relational_database_blueprint_id: String,
}

impl StoredRelationalDatabaseSnapshot {
    /// This record as the modelled `RelationalDatabaseSnapshot`.
    pub fn view(&self) -> crate::ops::types::RelationalDatabaseSnapshot {
        crate::ops::types::RelationalDatabaseSnapshot {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_RELATIONAL_DATABASE_SNAPSHOT.to_string()),
            tags: Some(self.tags.clone()),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            size_in_gb: Some(self.size_in_gb),
            state: Some(self.state.to_string()),
            from_relational_database_name: Some(self.from_relational_database_name.clone()),
            from_relational_database_arn: Some(self.from_relational_database_arn.clone()),
            from_relational_database_bundle_id: Some(
                self.from_relational_database_bundle_id.clone(),
            ),
            from_relational_database_blueprint_id: Some(
                self.from_relational_database_blueprint_id.clone(),
            ),
        }
    }
}
