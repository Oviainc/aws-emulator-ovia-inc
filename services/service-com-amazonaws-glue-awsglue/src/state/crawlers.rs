// SPDX-License-Identifier: CC0-1.0

//! The stored crawler record: what a create writes, what an update merges over it, and the
//! state a write leaves the crawler's schedule in.

use super::*;

/// The schedule state a write that STATES a crawler's schedule leaves it in: a cron
/// expression the write puts on the crawler is in force the moment the write returns, and
/// a crawler left with no expression has no schedule running for it at all.
///
/// One place, so `CreateCrawler`, `UpdateCrawler` and `UpdateCrawlerSchedule` cannot
/// disagree about the state the schedule they wrote is in.
pub(super) fn stated_schedule_state(expression: Option<&str>) -> String {
    if expression.is_some() {
        SCHEDULE_STATE_SCHEDULED.to_string()
    } else {
        SCHEDULE_STATE_NOT_SCHEDULED.to_string()
    }
}

/// A stored crawler.
///
/// Every member of the modelled `Crawler` shape that a create can fill is held here, so
/// a later `GetCrawler` reports what the create stored and nothing else: the members
/// only a crawl produces (`LastCrawl`, `CrawlElapsedTime`) are not stored, because no
/// crawl has run.
#[derive(Debug, Clone)]
pub struct CrawlerEntry {
    /// The crawler's name, which is also its key in [`ComAmazonawsGlueAwsglueState::crawlers`].
    pub name: String,
    pub role: String,
    pub targets: CrawlerTargets,
    pub database_name: Option<String>,
    pub description: Option<String>,
    /// The custom classifiers associated with the crawler; empty when the create named
    /// none, which is the accurate report for a crawler that has no custom classifier.
    pub classifiers: Vec<String>,
    pub table_prefix: Option<String>,
    pub schema_change_policy: Option<SchemaChangePolicy>,
    pub recrawl_policy: Option<RecrawlPolicy>,
    pub lineage_configuration: Option<LineageConfiguration>,
    pub lake_formation_configuration: Option<LakeFormationConfiguration>,
    pub configuration: Option<String>,
    pub crawler_security_configuration: Option<String>,
    /// The cron expression the create supplied, if any. Reported inside a `Schedule`.
    pub schedule_expression: Option<String>,
    /// The state that schedule is in, one of the modelled `ScheduleState` values: a cron
    /// expression a write states is in force ([`SCHEDULE_STATE_SCHEDULED`]) until the
    /// operation that suspends a schedule moves it to [`SCHEDULE_STATE_NOT_SCHEDULED`],
    /// which is also where a crawler carrying no expression sits. Reported as the
    /// `Schedule`'s `State`, so it is observable only while an expression is there to
    /// report it with.
    pub schedule_state: String,
    /// The catalog the crawler writes its output to.
    pub catalog_id: String,
    /// One of the modelled `CrawlerState` values; [`CRAWLER_STATE_READY`] at create.
    pub state: String,
    /// Server-set version counter, [`FIRST_CRAWLER_VERSION`] for a fresh crawler.
    pub version: i64,
    /// Server-set, written once at create so every later read reports the same instant.
    pub creation_time: SystemTime,
    /// Server-set; moves with every write to this crawler.
    pub last_updated: SystemTime,
    /// The request's `Tags`, which are not part of the `Crawler` shape — they are read
    /// back through the tagging operations.
    pub tags: HashMap<String, String>,
}

impl CrawlerEntry {
    /// Build a stored crawler from a `CreateCrawler` request plus the server-derived
    /// members (`State`, `Version`, `CreationTime`, `LastUpdated`), none of which the
    /// client may set.
    pub fn from_create_input(
        input: &CreateCrawlerInput,
        catalog_id: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            name: input.name.clone(),
            role: input.role.clone(),
            targets: input.targets.clone(),
            database_name: input.database_name.clone(),
            description: input.description.clone(),
            classifiers: input.classifiers.clone().unwrap_or_default(),
            table_prefix: input.table_prefix.clone(),
            schema_change_policy: input.schema_change_policy.clone(),
            recrawl_policy: input.recrawl_policy.clone(),
            lineage_configuration: input.lineage_configuration.clone(),
            lake_formation_configuration: input.lake_formation_configuration.clone(),
            configuration: input.configuration.clone(),
            crawler_security_configuration: input.crawler_security_configuration.clone(),
            schedule_expression: input.schedule.clone(),
            schedule_state: stated_schedule_state(input.schedule.as_deref()),
            catalog_id: catalog_id.to_string(),
            state: CRAWLER_STATE_READY.to_string(),
            version: FIRST_CRAWLER_VERSION,
            creation_time: now,
            last_updated: now,
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Apply an `UpdateCrawler` request to this stored crawler, member by member.
    ///
    /// Every member the request CARRIES replaces the stored one; every member it OMITS
    /// is preserved. That is what the model states for the one member whose preservation
    /// it spells out — "if you omit this value, the existing value on the crawler is
    /// preserved" (`UpdateCrawlerRequest$CatalogId`) — and what the update's own
    /// `Description` requires: it targets the REMOVABLE description shape where the
    /// create targets the plain one, so sending an empty description is how a caller
    /// takes the description OFF a crawler, a distinction that exists only because
    /// omitting a member leaves it alone.
    ///
    /// `catalog_id` is the catalog the request named, already resolved and checked by the
    /// handler, or `None` when the request named none.
    ///
    /// The server-set members move with the write: `Version` advances by one (the first
    /// version of a crawler is [`FIRST_CRAWLER_VERSION`] and an update moves it on) and
    /// `LastUpdated` becomes `now`. The crawler's `Name`, `CreationTime`, `State` and
    /// tags are not this request's to change: `UpdateCrawlerRequest` has no member for
    /// any of them.
    pub fn apply_update(
        &mut self,
        input: &UpdateCrawlerInput,
        catalog_id: Option<String>,
        now: SystemTime,
    ) {
        if let Some(role) = input.role.as_ref() {
            self.role = role.clone();
        }
        if let Some(targets) = input.targets.as_ref() {
            self.targets = targets.clone();
        }
        if let Some(database_name) = input.database_name.as_ref() {
            self.database_name = Some(database_name.clone());
        }
        // The removable description: empty takes it off the crawler rather than storing
        // a description that is the empty string.
        if let Some(description) = input.description.as_ref() {
            self.description = (!description.is_empty()).then(|| description.clone());
        }
        if let Some(classifiers) = input.classifiers.as_ref() {
            self.classifiers = classifiers.clone();
        }
        if let Some(table_prefix) = input.table_prefix.as_ref() {
            self.table_prefix = Some(table_prefix.clone());
        }
        if let Some(schema_change_policy) = input.schema_change_policy.as_ref() {
            self.schema_change_policy = Some(schema_change_policy.clone());
        }
        if let Some(recrawl_policy) = input.recrawl_policy.as_ref() {
            self.recrawl_policy = Some(recrawl_policy.clone());
        }
        if let Some(lineage_configuration) = input.lineage_configuration.as_ref() {
            self.lineage_configuration = Some(lineage_configuration.clone());
        }
        if let Some(lake_formation_configuration) = input.lake_formation_configuration.as_ref() {
            self.lake_formation_configuration = Some(lake_formation_configuration.clone());
        }
        if let Some(configuration) = input.configuration.as_ref() {
            self.configuration = Some(configuration.clone());
        }
        if let Some(security_configuration) = input.crawler_security_configuration.as_ref() {
            self.crawler_security_configuration = Some(security_configuration.clone());
        }
        if let Some(schedule) = input.schedule.as_ref() {
            self.schedule_expression = Some(schedule.clone());
            self.schedule_state = stated_schedule_state(Some(schedule));
        }
        if let Some(catalog_id) = catalog_id {
            self.catalog_id = catalog_id;
        }
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// Apply an `UpdateCrawlerSchedule` request's schedule to this stored crawler.
    ///
    /// That request STATES the crawler's schedule rather than merging into it: the cron
    /// expression it carries becomes the crawler's schedule, and a request that carries
    /// NONE leaves the crawler with no schedule at all. `CrawlerName` only ADDRESSES the
    /// crawler, so the schedule is the one thing the request says anything about and its
    /// absence can only mean "no schedule" — which is also the one way a caller can take
    /// a schedule off a crawler, since `UpdateCrawler`'s own `Schedule` is a plain
    /// expression whose absence means "leave the crawler alone".
    ///
    /// The write moves the server-set members exactly as [`Self::apply_update`] does,
    /// because it writes the same member of the same crawler definition: `Version`
    /// advances by one and `LastUpdated` becomes `now`. Nothing else about the crawler is
    /// this request's to change.
    pub fn apply_schedule(&mut self, schedule: Option<&str>, now: SystemTime) {
        self.schedule_expression = schedule.map(str::to_string);
        self.schedule_state = stated_schedule_state(schedule);
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// Whether this crawler's schedule is RUNNING, i.e. its cron expression is in force
    /// ([`SCHEDULE_STATE_SCHEDULED`]). False both for a crawler whose schedule has been
    /// suspended and for one that carries no expression to run: neither has a scheduler
    /// running for it, which is what the operations that suspend and resume a schedule
    /// decide about.
    pub fn schedule_is_running(&self) -> bool {
        self.schedule_state == SCHEDULE_STATE_SCHEDULED
    }

    /// Suspend this crawler's schedule: the schedule state becomes
    /// [`SCHEDULE_STATE_NOT_SCHEDULED`], which is all `StopCrawlerSchedule` decides —
    /// "sets the schedule state of the specified crawler to NOT_SCHEDULED, but does not
    /// stop the crawler if it is already running".
    ///
    /// The cron expression STAYS on the crawler: suspending a schedule is not removing it
    /// (that is what a schedule write stating no expression does), so the expression is
    /// there to be put back in force. The crawler's run `State` is untouched for the
    /// reason the documentation gives, and `Version` with it: it counts the crawler's
    /// DEFINITION, which this call writes no member of. `LastUpdated` moves, because the
    /// crawler record did change.
    pub fn suspend_schedule(&mut self, now: SystemTime) {
        self.schedule_state = SCHEDULE_STATE_NOT_SCHEDULED.to_string();
        self.last_updated = now;
    }

    /// Whether a crawl of this crawler is IN PROGRESS, i.e. its run state is not the one
    /// modelled `CrawlerState` with no run in it ([`CRAWLER_STATE_READY`]): of the other
    /// two, [`CRAWLER_STATE_RUNNING`] is a crawl under way and [`CRAWLER_STATE_STOPPING`]
    /// one that has been asked to stop and has not finished, so each still has a crawl
    /// the service is working on.
    pub fn crawl_in_progress(&self) -> bool {
        self.state != CRAWLER_STATE_READY
    }

    /// Rebuild the modelled `CrawlerMetrics` shape from what is stored: "Metrics for a
    /// specified crawler".
    ///
    /// Every member is computed from what the store holds about THIS crawler — whether it
    /// has a crawl in progress, and what the crawls it has finished did:
    ///
    /// * "True if the crawler is still estimating how long it will take to complete this
    ///   run" (`StillEstimating`) is about a run in progress, and the service estimates a
    ///   run's remaining time from the runs that finished before it; a crawler here has
    ///   finished none, so a crawl in progress is one that is still being estimated and a
    ///   crawler with no crawl in progress has nothing to estimate.
    /// * `TimeLeftSeconds` is "the estimated time left to complete a running crawl", so
    ///   there is no estimate to report for a run still being estimated, and none at all
    ///   for a crawler that is not running.
    /// * `LastRuntimeSeconds` and `MedianRuntimeSeconds` are durations of this crawler's
    ///   runs, and `TablesCreated`/`TablesUpdated`/`TablesDeleted` the catalog tables its
    ///   crawls have written; no crawl has run, which is why [`Self::to_crawler`] reports
    ///   no `LastCrawl` either, so each is zero.
    pub fn to_crawler_metrics(&self) -> CrawlerMetrics {
        CrawlerMetrics {
            crawler_name: Some(self.name.clone()),
            time_left_seconds: 0.0,
            still_estimating: self.crawl_in_progress(),
            last_runtime_seconds: 0.0,
            median_runtime_seconds: 0.0,
            tables_created: 0,
            tables_updated: 0,
            tables_deleted: 0,
        }
    }

    /// Rebuild the modelled `Crawler` output shape from what is stored.
    pub fn to_crawler(&self) -> Crawler {
        Crawler {
            name: Some(self.name.clone()),
            role: Some(self.role.clone()),
            targets: Some(self.targets.clone()),
            database_name: self.database_name.clone(),
            description: self.description.clone(),
            classifiers: Some(self.classifiers.clone()),
            recrawl_policy: self.recrawl_policy.clone(),
            schema_change_policy: self.schema_change_policy.clone(),
            lineage_configuration: self.lineage_configuration.clone(),
            state: Some(self.state.clone()),
            table_prefix: self.table_prefix.clone(),
            schedule: self.schedule_expression.as_ref().map(|expression| Schedule {
                schedule_expression: Some(expression.clone()),
                state: Some(self.schedule_state.clone()),
            }),
            // "If the crawler is running, contains the total time elapsed since the
            // last crawl began" — nothing has run, so no time has elapsed.
            crawl_elapsed_time: 0,
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated: Some(to_date_time(self.last_updated)),
            // "Status and error information about the most recent crawl": absent until
            // a crawl has produced one.
            last_crawl: None,
            version: self.version,
            configuration: self.configuration.clone(),
            crawler_security_configuration: self.crawler_security_configuration.clone(),
            lake_formation_configuration: self.lake_formation_configuration.clone(),
            catalog_id: Some(self.catalog_id.clone()),
        }
    }
}
