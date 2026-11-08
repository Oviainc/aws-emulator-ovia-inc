// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT'S CONFIGURATION SETS: the set of configuration sets one account holds, and
//! the options each of them carries that both Amazon SES APIs publish.
//!
//! Amazon SES publishes TWO APIs over ONE set of account configuration sets: the classic
//! `awsQuery` surface (`CreateConfigurationSet`, `DescribeConfigurationSet`,
//! `ListConfigurationSets`, `DeleteConfigurationSet`, the `…ConfigurationSetTrackingOptions`
//! and `PutConfigurationSetDeliveryOptions` sub-resource calls, and the two
//! `UpdateConfigurationSet…Enabled` switches) and the v2 REST surface
//! (`CreateConfigurationSet`, `GetConfigurationSet`, `ListConfigurationSets`,
//! `DeleteConfigurationSet` and the `PutConfigurationSet…Options` calls). A set recorded
//! through either one is the SAME account resource: the other surface describes it, refuses
//! a second create under its name, accepts a send that names it, and loses it when it is
//! deleted.
//!
//! So the set lives HERE, once in the shared engine, rather than in each surface's own store
//! — exactly as the account's identities do ([`super::ses_identities`]): a surface PUBLISHES
//! the sets it records and ADOPTS the ones it does not hold yet, and keeps beside that only
//! what its own API adds (the classic surface's event destinations; the v2 surface's
//! suppression, VDM, archiving and message-security options, its dedicated IP pool, its
//! tags).
//!
//! Nothing here names a wire shape. The registry holds the set as its creator spelled it and
//! the options the two models BOTH carry — the custom open-and-click redirect domain of its
//! tracking options, the TLS policy of its delivery options, whether reputation metrics are
//! collected, whether sending through the set is enabled, and when its reputation metrics
//! were last given a fresh start. Each surface spells those onto the shapes ITS model
//! declares, and a publication carries the whole set of them, so one surface's write is the
//! account's configuration rather than a value only that surface reports.
//!
//! The registry is BOUNDED at [`MAX_REGISTERED_CONFIGURATION_SETS`], the configuration-set
//! quota SES documents per account per Region, so a caller looping on a create cannot grow
//! it without end.

use std::collections::BTreeMap;
use std::time::SystemTime;

/// The configuration sets one account may hold, and so the ceiling on this registry: SES
/// documents a limit of 10,000 configuration sets per account per Region, and a publication
/// past it is refused rather than letting caller input grow the registry without end.
pub const MAX_REGISTERED_CONFIGURATION_SETS: usize = 10_000;

/// Which TLS policy a configuration set's delivery options carry.
///
/// The two SES APIs model the SAME two policies under DIFFERENT value sets — the classic
/// surface's `DeliveryOptions$TlsPolicy` carries `Require`/`Optional`, the v2 surface's
/// `REQUIRE`/`OPTIONAL` — so the registry holds the policy itself and each surface spells it
/// onto the value set ITS model declares, exactly as each spells the shared verification
/// state of an identity onto its own ([`super::ses_identities`]). A raw string would make
/// one surface report a value the other's model does not admit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SesTlsPolicy {
    /// The message is delivered only over a TLS connection.
    Require,
    /// The message may be delivered in plain text where TLS cannot be established.
    Optional,
}

impl SesTlsPolicy {
    /// The policy `value` names, whichever of the two surfaces' spellings it is written in,
    /// or `None` for a value neither model declares.
    ///
    /// The comparison is case-insensitive because the two value sets differ only in case,
    /// which is what lets one reading serve both surfaces.
    pub fn of(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("require") {
            return Some(Self::Require);
        }
        if value.eq_ignore_ascii_case("optional") {
            return Some(Self::Optional);
        }
        None
    }
}

/// One configuration set the account holds, as the registry records it: the facts both SES
/// APIs publish about a set.
///
/// A publisher states ALL of them, because both surfaces carry all of them: an absent
/// `Option` is a sub-resource the SET does not have (no tracking options, no delivery
/// options, no fresh start yet) rather than one the publishing API cannot express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SesConfigurationSetRecord {
    /// The set's name as the request that recorded it spelled it, which is the value both
    /// surfaces report in a listing and address the set by.
    pub name: String,
    /// The custom open-and-click redirect domain of the set's tracking options
    /// (`TrackingOptions$CustomRedirectDomain` on both surfaces), and `None` when the set
    /// has no tracking options.
    pub custom_redirect_domain: Option<String>,
    /// The TLS policy of the set's delivery options ([`SesTlsPolicy`], which both surfaces
    /// model under their own value set), and `None` when the set has no delivery options.
    pub tls_policy: Option<SesTlsPolicy>,
    /// Whether SES collects reputation metrics for the set
    /// (`ReputationOptions$ReputationMetricsEnabled`).
    pub reputation_metrics_enabled: bool,
    /// Whether sending through the set is enabled (`SendingOptions$SendingEnabled` on the v2
    /// surface, `UpdateConfigurationSetSendingEnabled` on the classic one).
    pub sending_enabled: bool,
    /// When the set's reputation metrics were last given a fresh start
    /// (`ReputationOptions$LastFreshStart`), and `None` while sending for the set has never
    /// been disabled and re-enabled.
    pub last_fresh_start: Option<SystemTime>,
}

/// The registry itself: configuration-set name -> the set the account holds under it.
///
/// Registered in the shared engine under THIS type, so the two SES surfaces reach one set of
/// configuration sets instead of each holding its own.
///
/// Keyed by the set's NAME, compared exactly: `ConfigurationSetName` is documented as
/// alphanumerics, hyphens and underscores, and nothing normalises one case into the other on
/// either surface. Ordered by it, so a listing either surface pages is in one stable order
/// for every caller — which is what makes its paging token resumable.
#[derive(Debug, Default)]
pub struct SesConfigurationSetDirectory {
    /// The records, by set name.
    sets: BTreeMap<String, SesConfigurationSetRecord>,
    /// When each set was first published, which is what a surface that ADOPTS a set reports
    /// as its creation instant rather than stamping the instant it happened to adopt it.
    created: BTreeMap<String, SystemTime>,
}

impl SesConfigurationSetDirectory {
    /// Publish `record` as a configuration set of this account, at `now`, and hand back the
    /// stored record.
    ///
    /// The publisher states the whole configuration ([`SesConfigurationSetRecord`]), so a
    /// set already registered takes it entire — that is what carries a sub-resource another
    /// surface DELETED, which no "leave what you have no value for" rule could express — and
    /// keeps only the instant it was first published.
    ///
    /// `None` when the registry already holds [`MAX_REGISTERED_CONFIGURATION_SETS`] sets and
    /// this is a new one: the caller refuses the request rather than growing the registry
    /// past the account's quota.
    pub fn publish(
        &mut self,
        record: SesConfigurationSetRecord,
        now: SystemTime,
    ) -> Option<&SesConfigurationSetRecord> {
        let name = record.name.clone();
        if !self.sets.contains_key(&name) {
            if self.sets.len() >= MAX_REGISTERED_CONFIGURATION_SETS {
                return None;
            }
            self.created.insert(name.clone(), now);
        }
        self.sets.insert(name.clone(), record);
        self.sets.get(&name)
    }

    /// Drop the set `name` names: it was deleted, so the account no longer holds it and
    /// neither surface reports it.
    ///
    /// Deleting a set the registry does not hold changes nothing, which is what makes a
    /// delete served on one surface and then replayed on the other harmless.
    pub fn forget(&mut self, name: &str) {
        self.sets.remove(name);
        self.created.remove(name);
    }

    /// The set `name` names, or `None` when the account holds no such configuration set.
    pub fn get(&self, name: &str) -> Option<&SesConfigurationSetRecord> {
        self.sets.get(name)
    }

    /// When the set `name` names was first published, or `None` when the account holds no
    /// such set.
    pub fn created_at(&self, name: &str) -> Option<SystemTime> {
        self.created.get(name).copied()
    }

    /// Every configuration set the account holds, ordered by name.
    pub fn records(&self) -> impl Iterator<Item = &SesConfigurationSetRecord> {
        self.sets.values()
    }

    /// How many configuration sets the account holds.
    pub fn len(&self) -> usize {
        self.sets.len()
    }

    /// Whether the account holds no configuration set at all.
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// Bring one surface's own configuration-set `table` and this registry back into
    /// agreement at `now`, so every API over the account's configuration sets answers for
    /// ONE set of them.
    ///
    /// The whole reconciliation, written ONCE here rather than per surface, so no two of
    /// them can resolve a hand-off differently. Four steps, in this order:
    ///
    /// 1. a set the table holds that the registry does not know about yet is PUBLISHED —
    ///    which is how a record written straight into the table (a test fixture) reaches the
    ///    other surfaces;
    /// 2. a set the table holds that the registry no longer holds was DELETED through
    ///    another surface, so it goes from the table too;
    /// 3. a set the registry holds that the table does not is ADOPTED
    ///    ([`AccountConfigurationSetRecord::adopt`]);
    /// 4. a set BOTH sides hold takes the options the account holds for it
    ///    ([`AccountConfigurationSetRecord::take_shared`]), which is how an option written
    ///    through one API becomes visible through the others instead of only the set itself.
    ///
    /// The work is proportional to the account's configuration-set count, which
    /// [`MAX_REGISTERED_CONFIGURATION_SETS`] bounds, and allocates only when there is a
    /// publication, an adoption or an option the two sides disagree on.
    pub fn reconcile<R: AccountConfigurationSetRecord>(
        &mut self,
        table: &mut BTreeMap<String, R>,
        now: SystemTime,
    ) {
        if table.values().any(|held| !held.is_registered()) {
            let unpublished: Vec<(String, SesConfigurationSetRecord)> = table
                .values()
                .filter(|held| !held.is_registered())
                .map(|held| (held.set_name().to_string(), held.published()))
                .collect();
            for (key, record) in unpublished {
                // A publication the registry refuses (the account is at its quota) leaves
                // the record unpublished rather than marking it registered: it is this
                // surface's own set, not one another surface deleted.
                if self.publish(record, now).is_none() {
                    continue;
                }
                if let Some(held) = table.get_mut(&key) {
                    held.mark_registered();
                }
            }
        }

        table.retain(|_, held| !held.is_registered() || self.get(held.set_name()).is_some());

        if self.records().any(|record| !table.contains_key(&record.name)) {
            let adopted: Vec<(String, R)> = self
                .records()
                .filter(|record| !table.contains_key(&record.name))
                .map(|record| {
                    let created = self.created_at(&record.name).unwrap_or(now);
                    (record.name.clone(), R::adopt(record, created))
                })
                .collect();
            for (key, record) in adopted {
                table.insert(key, record);
            }
        }

        // What the ACCOUNT holds for a set both sides already hold is the account's, not the
        // holding surface's: a record kept from an earlier call takes it, so an option
        // another API wrote — or a sub-resource it deleted — after this surface recorded or
        // adopted the set is reported here too instead of only on the API that wrote it.
        for held in table.values_mut() {
            if let Some(published) = self.sets.get(held.set_name()) {
                held.take_shared(published);
            }
        }
    }
}

/// One surface's OWN record of an account configuration set, as
/// [`SesConfigurationSetDirectory::reconcile`] reaches it.
///
/// Each API over the account's configuration sets keeps, beside the registry, what IT adds
/// to a set — the classic surface's event destinations; the v2 surface's suppression, VDM,
/// archiving and message-security options and its tags. This trait is the little each of
/// those records has to answer for the reconciliation to be written once: which set it is
/// for, whether the registry carries it yet, how the registry carries it, and how to make
/// one from a registry record.
///
/// A surface's table is keyed by the set's name, which is how both of them look a set up, so
/// there is no per-surface keying to state.
pub trait AccountConfigurationSetRecord: Sized {
    /// The configuration set this record is for, as the request that recorded it spelled it.
    fn set_name(&self) -> &str;

    /// Whether the registry already carries this set.
    ///
    /// This is what tells a set DELETED through another surface — registered here, gone from
    /// the registry — from one this surface recorded but has not published yet.
    fn is_registered(&self) -> bool;

    /// Note that the registry now carries this set.
    fn mark_registered(&mut self);

    /// Take onto this record the whole configuration the ACCOUNT holds for the set, as
    /// `published` holds it.
    ///
    /// Called for every set this surface and the registry BOTH hold, so an option another
    /// API wrote, or a sub-resource it deleted, after this surface recorded or adopted the
    /// set is reported here as well. What this surface's own API adds beside those options
    /// is untouched.
    ///
    /// Idempotent, and it allocates only when the two sides disagree.
    fn take_shared(&mut self, published: &SesConfigurationSetRecord);

    /// How the registry carries this record: the set's name and the whole configuration this
    /// surface now holds for it.
    fn published(&self) -> SesConfigurationSetRecord;

    /// This surface's record of a set the registry holds and it does not, which the account
    /// has held since `created_at`.
    ///
    /// Everything this surface's own API adds starts where its create starts it, since none
    /// of it has been asked for; the shared options are then taken on by
    /// [`Self::take_shared`], which runs for every set both sides hold.
    fn adopt(record: &SesConfigurationSetRecord, created_at: SystemTime) -> Self;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for one surface's own record: the set's name, the registry flag, the
    /// shared configuration it holds, and one member of its "own" that a reconciliation
    /// must leave alone.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Surface {
        name: String,
        registered: bool,
        custom_redirect_domain: Option<String>,
        sending_enabled: bool,
        own: Option<String>,
        created_at: SystemTime,
    }

    impl Surface {
        fn new(name: &str, created_at: SystemTime) -> Self {
            Self {
                name: name.to_string(),
                registered: false,
                custom_redirect_domain: None,
                sending_enabled: true,
                own: None,
                created_at,
            }
        }
    }

    impl AccountConfigurationSetRecord for Surface {
        fn set_name(&self) -> &str {
            &self.name
        }

        fn is_registered(&self) -> bool {
            self.registered
        }

        fn mark_registered(&mut self) {
            self.registered = true;
        }

        fn take_shared(&mut self, published: &SesConfigurationSetRecord) {
            self.custom_redirect_domain = published.custom_redirect_domain.clone();
            self.sending_enabled = published.sending_enabled;
        }

        fn published(&self) -> SesConfigurationSetRecord {
            SesConfigurationSetRecord {
                name: self.name.clone(),
                custom_redirect_domain: self.custom_redirect_domain.clone(),
                tls_policy: None,
                reputation_metrics_enabled: false,
                sending_enabled: self.sending_enabled,
                last_fresh_start: None,
            }
        }

        fn adopt(record: &SesConfigurationSetRecord, created_at: SystemTime) -> Self {
            Self {
                name: record.name.clone(),
                registered: true,
                custom_redirect_domain: None,
                sending_enabled: true,
                own: None,
                created_at,
            }
        }
    }

    fn record(name: &str) -> SesConfigurationSetRecord {
        SesConfigurationSetRecord {
            name: name.to_string(),
            custom_redirect_domain: None,
            tls_policy: Some(SesTlsPolicy::Require),
            reputation_metrics_enabled: false,
            sending_enabled: true,
            last_fresh_start: None,
        }
    }

    fn table(records: &[Surface]) -> BTreeMap<String, Surface> {
        records
            .iter()
            .map(|held| (held.name.clone(), held.clone()))
            .collect()
    }

    #[test]
    fn a_set_one_surface_recorded_is_published_and_adopted_by_the_other() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesConfigurationSetDirectory::default();
        let mut one = table(&[Surface::new("orders", now)]);
        directory.reconcile(&mut one, now);
        assert!(
            one["orders"].registered,
            "case published: expected the record to be marked registered, got {:?}",
            one["orders"]
        );
        assert_eq!(
            directory.get("orders").map(|held| held.name.clone()),
            Some("orders".to_string()),
            "case published: expected the registry to hold the set, got {:?}",
            directory.get("orders")
        );

        let mut other: BTreeMap<String, Surface> = BTreeMap::new();
        directory.reconcile(&mut other, now);
        assert_eq!(
            other.keys().cloned().collect::<Vec<String>>(),
            vec!["orders".to_string()],
            "case adopted: expected the other surface to adopt the set, got {:?}",
            other.keys().collect::<Vec<&String>>()
        );
        assert_eq!(
            other["orders"].created_at, now,
            "case adopted: expected the set's first publication instant, got {:?}",
            other["orders"].created_at
        );
    }

    #[test]
    fn an_option_written_on_one_surface_is_reported_by_the_other() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesConfigurationSetDirectory::default();
        let mut one = table(&[Surface::new("orders", now)]);
        directory.reconcile(&mut one, now);
        let mut other: BTreeMap<String, Surface> = BTreeMap::new();
        directory.reconcile(&mut other, now);

        // One surface writes two shared options and publishes the set as it now holds it.
        let held = one.get_mut("orders").expect("the recorded set");
        held.custom_redirect_domain = Some("click.example.com".to_string());
        held.sending_enabled = false;
        held.own = Some("this surface only".to_string());
        directory.publish(held.published(), now);

        directory.reconcile(&mut other, now);
        assert_eq!(
            other["orders"].custom_redirect_domain.as_deref(),
            Some("click.example.com"),
            "case shared option: expected the written domain, got {:?}",
            other["orders"].custom_redirect_domain
        );
        assert!(
            !other["orders"].sending_enabled,
            "case shared option: expected sending disabled, got {:?}",
            other["orders"]
        );
        assert_eq!(
            other["orders"].own, None,
            "case own member: expected the other surface's own member untouched, got {:?}",
            other["orders"].own
        );

        // And a sub-resource the FIRST surface then deletes is gone from the second too,
        // which is what carries a delete rather than only a write.
        let held = one.get_mut("orders").expect("the recorded set");
        held.custom_redirect_domain = None;
        directory.publish(held.published(), now);
        directory.reconcile(&mut other, now);
        assert_eq!(
            other["orders"].custom_redirect_domain, None,
            "case deleted sub-resource: expected no redirect domain, got {:?}",
            other["orders"].custom_redirect_domain
        );
    }

    #[test]
    fn a_deleted_set_goes_from_every_surface() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesConfigurationSetDirectory::default();
        let mut one = table(&[Surface::new("orders", now)]);
        directory.reconcile(&mut one, now);
        let mut other: BTreeMap<String, Surface> = BTreeMap::new();
        directory.reconcile(&mut other, now);

        directory.forget("orders");
        directory.reconcile(&mut other, now);
        assert!(
            other.is_empty(),
            "case deleted: expected the set gone from the other surface, got {:?}",
            other.keys().collect::<Vec<&String>>()
        );
        directory.reconcile(&mut one, now);
        assert!(
            one.is_empty(),
            "case deleted: expected the set gone from the recording surface, got {:?}",
            one.keys().collect::<Vec<&String>>()
        );
    }

    #[test]
    fn the_registry_is_bounded() {
        let now = SystemTime::UNIX_EPOCH;
        let mut directory = SesConfigurationSetDirectory::default();
        for index in 0..MAX_REGISTERED_CONFIGURATION_SETS {
            assert!(
                directory.publish(record(&format!("set-{index}")), now).is_some(),
                "case under the quota: expected set-{index} to publish"
            );
        }
        assert!(
            directory.publish(record("one-too-many"), now).is_none(),
            "case at the quota: expected the publication to be refused, got {:?}",
            directory.get("one-too-many")
        );
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_CONFIGURATION_SETS,
            "case at the quota: expected the registry to stay at its ceiling, got {}",
            directory.len()
        );
    }

    #[test]
    fn a_publication_keeps_the_instant_the_set_was_first_published() {
        let first = SystemTime::UNIX_EPOCH;
        let later = first + std::time::Duration::from_secs(60);
        let mut directory = SesConfigurationSetDirectory::default();
        directory.publish(record("orders"), first);
        directory.publish(record("orders"), later);
        assert_eq!(
            directory.created_at("orders"),
            Some(first),
            "case re-published: expected the first publication instant, got {:?}",
            directory.created_at("orders")
        );
    }
}
