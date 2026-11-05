// SPDX-License-Identifier: CC0-1.0

//! The account's ONE credential report: how long a generated one stays readable, and the
//! `text/csv` artifact itself.
//!
//! `GenerateCredentialReport` starts a generation and `GetCredentialReport` reads what the
//! generation produced, so what a REPORT IS belongs to neither operation: it is stated
//! here once — the window a report stays readable for
//! ([`CREDENTIAL_REPORT_VALIDITY_SECONDS`]), and the rendering of the account's own
//! credentials it carries
//! ([`ComAmazonawsIamAwsidentitymanagementv20100508State::credential_report_csv`]) — so
//! the two operations cannot describe one account differently.
//!
//! The artifact is rendered ONCE, by the call that starts the generation, and kept on the
//! generation record ([`CredentialReportGeneration::content`]): a report is a SNAPSHOT of
//! the account's credentials, so every read of one report answers the same bytes and the
//! same instant, and reading one changes nothing a later caller sees.

#[allow(unused_imports)]
use super::*;

/// How long a generated credential report stays readable.
///
/// The published rule is one window, not two: "IAM first checks whether a report for the
/// Amazon Web Services account has been generated within the past four hours. If so, the
/// report is downloaded" and otherwise a new report has to be generated ("Getting
/// credential reports for your Amazon Web Services account", IAM User Guide). So a report
/// is downloadable for exactly as long as it keeps the account from generating another
/// ([`CREDENTIAL_REPORT_REGENERATION_SECONDS`]), which is why this names that one figure
/// rather than a second one that could drift from it: from its end a read is refused
/// `CredentialReportExpiredException` and a fresh `GenerateCredentialReport` is what the
/// account needs, which is exactly when that operation starts one.
pub const CREDENTIAL_REPORT_VALIDITY_SECONDS: i64 = CREDENTIAL_REPORT_REGENERATION_SECONDS;

/// The `user` cell of the row a credential report reports the account's ROOT USER in.
///
/// The root user has no friendly name of its own, so the report names it by this fixed
/// literal — "the AWS account root user ... is listed as `<root_account>`" ("Getting
/// credential reports for your Amazon Web Services account", IAM User Guide).
pub const CREDENTIAL_REPORT_ROOT_USER: &str = "<root_account>";

/// The columns of a credential report, in the published order.
///
/// Source: the report-content column list in "Getting credential reports for your Amazon
/// Web Services account" (IAM User Guide), which names each column and what it carries.
/// The per-credential columns repeat once per credential the account's quota admits — two
/// access keys ([`MAX_ACCESS_KEYS_PER_USER`]) and two signing certificates
/// ([`MAX_SIGNING_CERTIFICATES_PER_USER`]) — and the slots are numbered from 1, so the
/// list below is the identity columns followed by those repeats.
pub const CREDENTIAL_REPORT_COLUMNS: &[&str] = &[
    "user",
    "arn",
    "user_creation_time",
    "password_enabled",
    "password_last_used",
    "password_last_changed",
    "password_next_rotation",
    "mfa_active",
    "access_key_1_active",
    "access_key_1_last_rotated",
    "access_key_1_last_used_date",
    "access_key_1_last_used_region",
    "access_key_1_last_used_service",
    "access_key_2_active",
    "access_key_2_last_rotated",
    "access_key_2_last_used_date",
    "access_key_2_last_used_region",
    "access_key_2_last_used_service",
    "cert_1_active",
    "cert_1_last_rotated",
    "cert_2_active",
    "cert_2_last_rotated",
];

/// What a cell carries when the column does not apply to the row at all: there is no such
/// credential, or no value was ever recorded for it. "An entry of `N/A`" is the published
/// literal for that case (IAM User Guide, as [`CREDENTIAL_REPORT_COLUMNS`]).
const NOT_APPLICABLE: &str = "N/A";

/// What a cell carries when the column applies but the account holds no observation for
/// it — a password that has never been used to sign in. The published literal for "no
/// sign-in data" (IAM User Guide, as [`CREDENTIAL_REPORT_COLUMNS`]).
const NO_INFORMATION: &str = "no_information";

/// What a cell carries when the column does not exist for the row's kind of principal:
/// the account root user has no IAM-managed password, so the three password-configuration
/// columns carry this published literal for its row (IAM User Guide, as
/// [`CREDENTIAL_REPORT_COLUMNS`]).
const NOT_SUPPORTED: &str = "not_supported";

/// A true boolean cell, as the report spells one.
const REPORTED_TRUE: &str = "true";

/// A false boolean cell, as the report spells one.
const REPORTED_FALSE: &str = "false";

/// The UTC offset every instant in a report is spelled with.
///
/// A report's instants are "in ISO 8601 date-time format" (IAM User Guide, as
/// [`CREDENTIAL_REPORT_COLUMNS`]) written with the numeric zero offset — the spelling
/// `datetime.isoformat()` of a UTC instant produces, which is what a client parsing a cell
/// reads — rather than with the `Z` designator the modelled timestamp members carry.
const REPORTED_UTC_OFFSET: &str = "+00:00";

/// The ARN of the account's root user: `arn:aws:iam::<account>:root`.
///
/// Built through the shared ARN builder, so the account segment is the one every other ARN
/// this emulator emits carries; IAM is a global service, so the Region segment is empty,
/// exactly as it is for [`user_arn`].
#[must_use]
pub fn account_root_arn() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// `at` as a credential report spells an instant: whole seconds, with the numeric UTC
/// offset ([`REPORTED_UTC_OFFSET`]).
fn reported_instant(at: ::aws_smithy_types::DateTime) -> String {
    let whole = ::aws_smithy_types::DateTime::from_secs(at.secs());
    let spelled = whole
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    match spelled.strip_suffix('Z') {
        Some(civil) => format!("{civil}{REPORTED_UTC_OFFSET}"),
        None => spelled,
    }
}

/// `flag` as a credential report spells a boolean cell.
fn reported_flag(flag: bool) -> &'static str {
    if flag {
        REPORTED_TRUE
    } else {
        REPORTED_FALSE
    }
}

/// One cell, owned, so a row is built from literals and rendered values alike.
fn cell(value: &str) -> String {
    value.to_string()
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The account's credential report as the `text/csv` artifact
    /// `GetCredentialReport$Content` carries: the header row
    /// ([`CREDENTIAL_REPORT_COLUMNS`]), the account's root user, then one row per IAM user
    /// the account holds, in the order this store enumerates users — the same order its
    /// list operations page over, so two reports of one account are byte-identical.
    ///
    /// `key_uses` is what the account recorded about each of its access keys being used
    /// (`crate::service::management_events::last_use_by_access_key`), the same fact
    /// `GetAccessKeyLastUsed` reports for one key: the usage columns are read off it rather
    /// than invented, and a key nothing was recorded against carries [`NOT_APPLICABLE`].
    ///
    /// A READ of this store: every cell is a value the account already holds. `None` when
    /// the report could not be written at all ([`render_csv`]), which is no report rather
    /// than an empty one: a caller must never be handed a file it cannot tell from the
    /// report of an account holding nothing.
    pub fn credential_report_csv(
        &self,
        key_uses: &BTreeMap<String, crate::service::management_events::AccessKeyUse>,
    ) -> Option<String> {
        let mut rows = vec![self.root_account_row()];
        for (user_key, user) in &self.users {
            rows.push(self.user_row(user_key, user, key_uses));
        }
        render_csv(&rows)
    }

    /// The row the account's ROOT USER is reported in.
    ///
    /// The root user is the account itself rather than one of its IAM entities, so nothing
    /// in this store holds credentials for it: the three password-configuration columns
    /// carry [`NOT_SUPPORTED`], the password has no recorded sign-in
    /// ([`NO_INFORMATION`]), and it holds no MFA device, no access key and no signing
    /// certificate — each reported by the same empty-slot cells any other row without one
    /// carries.
    fn root_account_row(&self) -> Vec<String> {
        let mut row = vec![
            cell(CREDENTIAL_REPORT_ROOT_USER),
            account_root_arn(),
            reported_instant(::aws_smithy_types::DateTime::from_millis(
                ACCOUNT_BOOTSTRAP_CREATE_MILLIS,
            )),
            cell(NOT_SUPPORTED),
            cell(NO_INFORMATION),
            cell(NOT_SUPPORTED),
            cell(NOT_SUPPORTED),
            cell(reported_flag(false)),
        ];
        row.extend(empty_access_key_slots(MAX_ACCESS_KEYS_PER_USER));
        row.extend(empty_certificate_slots(MAX_SIGNING_CERTIFICATES_PER_USER));
        row
    }

    /// The row one IAM user is reported in: what the account holds about that user's
    /// console password, its MFA devices, its access keys and its signing certificates.
    fn user_row(
        &self,
        user_key: &str,
        user: &User,
        key_uses: &BTreeMap<String, crate::service::management_events::AccessKeyUse>,
    ) -> Vec<String> {
        // "A login profile is created when you create a password for the user"
        // (`GetLoginProfile`), so the profile the account holds for a user IS that user's
        // console password — and a user without one has no password columns to report.
        let password = self.login_profiles.get(user_key);
        let mut row = vec![
            user.user_name.clone(),
            user.arn.clone(),
            reported_instant(user.create_date),
            cell(reported_flag(password.is_some())),
            match (password, user.password_last_used) {
                (None, _) => cell(NOT_APPLICABLE),
                (Some(_), None) => cell(NO_INFORMATION),
                (Some(_), Some(used)) => reported_instant(used),
            },
            // The instant the account holds for the password in force: "the CreateDate
            // reflects the initial password creation date" (`GetLoginProfile`), which is
            // the one instant this store keeps for a console password.
            match password {
                None => cell(NOT_APPLICABLE),
                Some(profile) => reported_instant(profile.create_date),
            },
            self.password_next_rotation(password),
            cell(reported_flag(self.holds_an_mfa_device(user_key))),
        ];
        row.extend(self.access_key_slots(user_key, key_uses));
        row.extend(self.certificate_slots(user_key));
        row
    }

    /// When the user holding `password` must set a new one: the instant its password was
    /// set plus the account's own maximum password age, when the account's password policy
    /// sets one ("the number of days that an IAM user password is valid",
    /// `PasswordPolicy$MaxPasswordAge`). [`NOT_APPLICABLE`] for a user with no password,
    /// and for an account whose policy requires no rotation — the documented default, under
    /// which "IAM user passwords never expire".
    fn password_next_rotation(&self, password: Option<&LoginProfileRecord>) -> String {
        let rotation = self
            .account_password_policy
            .as_ref()
            .and_then(|policy| policy.max_password_age);
        match (password, rotation) {
            (Some(profile), Some(days)) => {
                reported_instant(::aws_smithy_types::DateTime::from_secs(
                    profile
                        .create_date
                        .secs()
                        .saturating_add(i64::from(days).saturating_mul(SECONDS_PER_DAY)),
                ))
            }
            _ => cell(NOT_APPLICABLE),
        }
    }

    /// Whether the user keyed `user_key` has an MFA device enabled: either a virtual MFA
    /// device the account holds for it, or a FIDO security key — both are "an MFA device …
    /// enabled" for the user, so the column is true for either.
    fn holds_an_mfa_device(&self, user_key: &str) -> bool {
        self.virtual_mfa_devices
            .values()
            .any(|device| device.user_key.as_deref() == Some(user_key))
            || self
                .fido_security_keys
                .values()
                .any(|device| device.user_key == user_key)
    }

    /// The access-key columns of one user's row: the five cells of each slot the quota
    /// admits ([`MAX_ACCESS_KEYS_PER_USER`]), oldest key first, with the slots the user
    /// holds no key in reported as empty.
    ///
    /// Ordered by the instant each key was minted, the key id breaking a tie, so a key
    /// keeps its slot across two reports of one account.
    fn access_key_slots(
        &self,
        user_key: &str,
        key_uses: &BTreeMap<String, crate::service::management_events::AccessKeyUse>,
    ) -> Vec<String> {
        let mut held: Vec<(&String, &AccessKeyRecord)> = self
            .access_keys
            .iter()
            .filter(|(_, record)| record.user_key == user_key)
            .collect();
        held.sort_by_key(|(access_key_id, record)| {
            (
                record.create_date.secs(),
                record.create_date.subsec_nanos(),
                *access_key_id,
            )
        });

        let mut cells = Vec::new();
        for slot in 0..MAX_ACCESS_KEYS_PER_USER {
            match held.get(slot) {
                None => cells.extend(empty_access_key_slots(1)),
                Some((access_key_id, record)) => {
                    cells.push(cell(reported_flag(
                        record.status == ACCESS_KEY_ACTIVE_STATUS,
                    )));
                    cells.push(reported_instant(record.create_date));
                    match key_uses.get(*access_key_id) {
                        None => cells.extend(empty_usage_cells()),
                        Some(used) => {
                            cells.push(reported_instant(::aws_smithy_types::DateTime::from(
                                used.at,
                            )));
                            cells.push(used.region.clone());
                            cells.push(used.service_name.clone());
                        }
                    }
                }
            }
        }
        cells
    }

    /// The signing-certificate columns of one user's row: the two cells of each slot the
    /// quota admits ([`MAX_SIGNING_CERTIFICATES_PER_USER`]), oldest certificate first,
    /// ordered and padded exactly as [`Self::access_key_slots`] orders and pads the key
    /// slots.
    fn certificate_slots(&self, user_key: &str) -> Vec<String> {
        let mut held: Vec<(&String, &SigningCertificateRecord)> = self
            .signing_certificates
            .iter()
            .filter(|(_, record)| record.user_key == user_key)
            .collect();
        held.sort_by_key(|(certificate_id, record)| {
            (
                record.upload_date.secs(),
                record.upload_date.subsec_nanos(),
                *certificate_id,
            )
        });

        let mut cells = Vec::new();
        for slot in 0..MAX_SIGNING_CERTIFICATES_PER_USER {
            match held.get(slot) {
                None => cells.extend(empty_certificate_slots(1)),
                Some((_, record)) => {
                    cells.push(cell(reported_flag(
                        record.status == CREDENTIAL_STATUS_ACTIVE,
                    )));
                    cells.push(reported_instant(record.upload_date));
                }
            }
        }
        cells
    }
}

/// The three usage cells of an access key nothing was recorded against: the key exists but
/// has not been used, which every one of them reports as [`NOT_APPLICABLE`].
fn empty_usage_cells() -> Vec<String> {
    vec![
        cell(NOT_APPLICABLE),
        cell(NOT_APPLICABLE),
        cell(NOT_APPLICABLE),
    ]
}

/// `slots` access-key slots a row holds no key in: the slot is reported inactive, and the
/// instant columns carry [`NOT_APPLICABLE`] because there is no key to date.
fn empty_access_key_slots(slots: usize) -> Vec<String> {
    let mut cells = Vec::new();
    for _ in 0..slots {
        cells.push(cell(reported_flag(false)));
        cells.push(cell(NOT_APPLICABLE));
        cells.extend(empty_usage_cells());
    }
    cells
}

/// `slots` signing-certificate slots a row holds no certificate in, reported the way
/// [`empty_access_key_slots`] reports an empty key slot.
fn empty_certificate_slots(slots: usize) -> Vec<String> {
    let mut cells = Vec::new();
    for _ in 0..slots {
        cells.push(cell(reported_flag(false)));
        cells.push(cell(NOT_APPLICABLE));
    }
    cells
}

/// The report file: the header row ([`CREDENTIAL_REPORT_COLUMNS`]) followed by `rows`.
///
/// Comma-separated values as the `csv` crate writes them, so the quoting of a cell that
/// needs it — an IAM name may carry a comma, since `^[\w+=,.@-]+$` admits one — is the
/// codec's rule and not re-derived here. A row holds no byte the codec can refuse and every
/// row carries one cell per column, so the writer cannot fail; an unexpected failure is
/// `None` — no report — rather than a truncated or empty file a client could not tell from
/// a complete one, and is logged.
fn render_csv(rows: &[Vec<String>]) -> Option<String> {
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    let mut write = || -> Result<(), ::csv::Error> {
        writer.write_record(CREDENTIAL_REPORT_COLUMNS)?;
        for row in rows {
            writer.write_record(row)?;
        }
        writer.flush().map_err(::csv::Error::from)
    };
    if let Err(error) = write() {
        ::tracing::warn!(%error, "the account's credential report could not be written");
        return None;
    }
    let file = writer
        .into_inner()
        .map_err(|error| {
            ::tracing::warn!(%error, "the account's credential report could not be written");
        })
        .ok()?;
    Some(String::from_utf8_lossy(&file).into_owned())
}
