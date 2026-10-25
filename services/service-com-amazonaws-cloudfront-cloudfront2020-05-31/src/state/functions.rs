// SPDX-License-Identifier: CC0-1.0

//! The CloudFront FUNCTION slice of this service's state.
//!
//! A CloudFront function is its own resource kind, so everything about how one is
//! persisted and projected lives here rather than in the parent module: the two stages a
//! function has, the record each stage carries, the published quotas a create is refused
//! at, the ARN and the path a function is addressed by, and the modelled errors the
//! function operations answer with. The parent
//! [`ComAmazonawsCloudfrontCloudfront20200531State`] holds the one map
//! (`functions`) and this module holds everything that reads or writes it, so the
//! function surface can grow without touching the parts of the store other resource
//! kinds own.
//!
//! Nothing in here is owed by the CLOCK: a function CloudFront accepts exists in
//! `DEVELOPMENT` immediately and reaches `LIVE` only through a `PublishFunction` CALL, so
//! no status here is transient and the parent's `settle` has nothing to do for a
//! function.

use std::time::SystemTime;

use crate::ops::types::{
    DistributionConfig, FunctionConfig, FunctionMetadata, FunctionSummary, NoSuchFunctionExists,
};

use super::{
    items_len, quantity_disagreement, ComAmazonawsCloudfrontCloudfront20200531State,
    StoredKeyValueStore, TagSet,
};

/// `FunctionMetadata$Stage` for the stage a function is created in — "When you create a
/// function, it's in the `DEVELOPMENT` stage. In this stage, you can test the function
/// with `TestFunction`, and update it with `UpdateFunction`." One of the two values the
/// modelled `FunctionStage` enum admits.
pub const FUNCTION_STAGE_DEVELOPMENT: &str = "DEVELOPMENT";

/// `FunctionMetadata$Stage` for the published stage — "When a function is in the `LIVE`
/// stage, you can attach the function to a distribution's cache behavior, using the
/// function's ARN." The other value the modelled `FunctionStage` enum admits; a function
/// reaches it only through `PublishFunction`.
pub const FUNCTION_STAGE_LIVE: &str = "LIVE";

/// The stage a `FunctionStage` member that was left out means.
///
/// `DescribeFunction$Stage` and `GetFunction$Stage` are optional, and CloudFront
/// documents the stage a function starts life in as the one a request that names none
/// addresses: "When you create a function, it's in the `DEVELOPMENT` stage."
pub const DEFAULT_FUNCTION_STAGE: &str = FUNCTION_STAGE_DEVELOPMENT;

/// `FunctionSummary$Status` for a function whose stage has not been published.
///
/// A function CloudFront has just created exists only in `DEVELOPMENT` and has never been
/// copied to `LIVE`, which is what this status reports. Nothing about it is owed by the
/// CLOCK: a function becomes published by a `PublishFunction` CALL, never by waiting, so
/// this status stands until such a call.
pub const FUNCTION_STATUS_UNPUBLISHED: &str = "UNPUBLISHED";

/// `FunctionSummary$Status` for a published stage that nothing serving traffic names.
///
/// A publish COPIES one stage into the other, and what a published stage reports from then
/// on is whether anything uses it: CloudFront distinguishes a function that is live but
/// attached to nothing from one a distribution runs. The model targets `Status` at plain
/// `string` and states no value list, so the two words are spelled out here once, beside
/// [`FUNCTION_STATUS_UNPUBLISHED`], and every publish reports one of them computed from the
/// stored distributions rather than from the request being served.
pub const FUNCTION_STATUS_UNASSOCIATED: &str = "UNASSOCIATED";

/// `FunctionSummary$Status` for a published stage a stored distribution names — the other
/// value of the pair [`FUNCTION_STATUS_UNASSOCIATED`] documents.
pub const FUNCTION_STATUS_ASSOCIATED: &str = "ASSOCIATED";

/// The most CloudFront functions this emulator holds, refused at with the modelled
/// `TooManyFunctions` ("You have reached the maximum number of CloudFront functions for
/// this Amazon Web Services account").
///
/// CloudFront's published quota for "Functions per Amazon Web Services account" is 100,
/// and the quota page is what the error itself points at, so the bound is that number
/// rather than one chosen here. It is an entry count checked before anything is minted:
/// a function record is what a later read answers from, so it cannot be evicted.
pub const MAX_FUNCTIONS: usize = 100;

/// The largest function code this emulator accepts, refused at with the modelled
/// `FunctionSizeLimitExceeded` ("The function is too large").
///
/// CloudFront's published quota for the "Maximum size of the function code and included
/// libraries" is 10 KB, while `FunctionBlob` admits up to 40960 bytes on the wire — which
/// is exactly what makes this refusal reachable: the boundary validator rejects anything
/// past the modelled bound, and this quota rejects the code that is within the model but
/// past what CloudFront runs.
pub const MAX_FUNCTION_CODE_BYTES: usize = 10 * 1024;

/// The most key value stores one function may be associated with — CloudFront's published
/// quota for "Key value stores per function" is 1 ("You can associate a single key value
/// store to a function"). A configuration naming more describes no function CloudFront
/// runs, so it is refused with the modelled `InvalidArgument`.
pub const MAX_KEY_VALUE_STORE_ASSOCIATIONS: usize = 1;

/// One stage of one CloudFront function, as this emulator persists it.
///
/// A function's two stages — `DEVELOPMENT` and `LIVE` — each carry their OWN code,
/// configuration, status and version token: "use `PublishFunction` to copy the function
/// from the `DEVELOPMENT` stage to `LIVE`" is a copy, so an edit to one stage is not an
/// edit to the other, and each stage answers reads with what was last copied or written
/// into it.
#[derive(Debug, Clone)]
pub struct StoredFunctionStage {
    /// Which stage this record is: [`FUNCTION_STAGE_DEVELOPMENT`] or
    /// [`FUNCTION_STAGE_LIVE`], reported as `FunctionMetadata$Stage`.
    pub stage: String,
    /// The configuration as submitted — "Configuration information about the function,
    /// including an optional comment and the function's runtime." Stored whole, so a read
    /// reports what was stored rather than re-deriving anything from a request.
    pub config: FunctionConfig,
    /// The function code as submitted ("The function code"), which `GetFunction` answers
    /// with byte for byte.
    pub code: Vec<u8>,
    /// `FunctionSummary$Status` — "The status of the CloudFront function."
    pub status: String,
    /// The version token the last mutation of this stage handed back, and the single
    /// source of truth for every `If-Match` check against it.
    pub etag: String,
    /// `FunctionMetadata$CreatedTime` — "The date and time when the function was
    /// created."
    pub created_time: SystemTime,
    /// `FunctionMetadata$LastModifiedTime` — "The date and time when the function was
    /// most recently updated."
    pub last_modified_time: SystemTime,
}

impl StoredFunctionStage {
    /// The `FunctionSummary` projection every function operation returns for this stage,
    /// read from the one stored record.
    ///
    /// The function's name and ARN belong to the function rather than to one stage, so
    /// they are passed in by [`StoredFunction`]: both stages of one function report the
    /// SAME ARN, which is what lets a cache behavior name a function by ARN.
    pub fn to_summary(&self, name: &str, arn: &str) -> FunctionSummary {
        FunctionSummary {
            name: name.to_string(),
            status: Some(self.status.clone()),
            function_config: self.config.clone(),
            function_metadata: FunctionMetadata {
                function_arn: arn.to_string(),
                stage: Some(self.stage.clone()),
                created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
                last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            },
        }
    }
}

/// One CloudFront function, as this emulator persists it.
///
/// The record is the single source of truth for the function's name, the ARN minted for
/// it, each stage's code, configuration, status, timestamps and version token, and the
/// tags it carries — so the create and every later read answer from it rather than from
/// the request being served.
#[derive(Debug, Clone)]
pub struct StoredFunction {
    /// `FunctionSummary$Name` — "A name to identify the function." Unique per account, so
    /// it is also the key this function is held under.
    pub name: String,
    /// `FunctionMetadata$FunctionARN` — "The ARN uniquely identifies the function."
    /// Minted at creation and the same for both stages.
    pub arn: String,
    /// The `DEVELOPMENT` stage, which exists for the whole life of the function: "When
    /// you create a function, it's in the `DEVELOPMENT` stage."
    pub development: StoredFunctionStage,
    /// The `LIVE` stage, which exists only once `PublishFunction` has copied the
    /// `DEVELOPMENT` stage into it — `None` until then, which is what makes a read of the
    /// `LIVE` stage of a never-published function a function that does not exist.
    pub live: Option<StoredFunctionStage>,
    /// The tags submitted with the create, in the SAME representation every tagged
    /// resource of this service uses, so one tagging rule covers them all.
    pub tags: TagSet,
}

impl StoredFunction {
    /// The stage `stage` names, or `None` when this function has no such stage — either
    /// because it has never been published, or because the value names no stage at all.
    ///
    /// The one lookup every function read goes through: the `FunctionSummary` and the
    /// version token a read reports both come from the stage record it answers.
    pub fn stage(&self, stage: &str) -> Option<&StoredFunctionStage> {
        match stage {
            FUNCTION_STAGE_DEVELOPMENT => Some(&self.development),
            FUNCTION_STAGE_LIVE => self.live.as_ref(),
            _ => None,
        }
    }
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// The function `name` names, or `None` when the store holds none.
    pub fn function(&self, name: &str) -> Option<&StoredFunction> {
        self.functions.get(name)
    }

    /// The function `arn` names, or `None` when this store holds none.
    ///
    /// The ARN a function carries is the one minted when it was created, so the lookup
    /// compares against the STORED value rather than re-deriving an identifier out of the
    /// request. ONE ARN lookup for this resource kind, which the tag read goes through.
    pub fn function_by_arn(&self, arn: &str) -> Option<&StoredFunction> {
        self.functions.values().find(|function| function.arn == arn)
    }

    /// The key value store `arn` names, or `None` when this store holds none.
    ///
    /// `KeyValueStoreAssociation$KeyValueStoreARN` names a store by the ARN its own
    /// create minted, so the lookup compares against the STORED ARN rather than
    /// re-deriving an identifier out of the request — which is what decides whether a
    /// function configuration may associate it.
    pub fn key_value_store_by_arn(&self, arn: &str) -> Option<&StoredKeyValueStore> {
        self.key_value_stores.values().find(|store| store.arn == arn)
    }

    /// The reason `config` describes no function this service can hold, or `None` when it
    /// describes one.
    ///
    /// ONE check for every payload that carries a `FunctionConfig` — a create and an
    /// update both store a whole configuration, so both refuse the same configurations:
    ///
    /// * a list wrapper whose `Quantity` disagrees with its `Items`
    ///   ([`function_config_quantity_mismatch`]);
    /// * more key value store associations than a function may carry
    ///   ([`MAX_KEY_VALUE_STORE_ASSOCIATIONS`]);
    /// * an association naming a key value store this service does not hold — the ARN is
    ///   compared against the STORED ARN of a key value store, never taken on trust from
    ///   the request.
    ///
    /// The caller reports the reason as the `InvalidArgument` its operation models for an
    /// unusable argument.
    pub fn function_config_unusable(&self, config: &FunctionConfig) -> Option<String> {
        if let Some(message) = function_config_quantity_mismatch(config) {
            return Some(message);
        }

        let associations = config
            .key_value_store_associations
            .as_ref()
            .map_or(&[][..], |associations| {
                associations.items.as_deref().unwrap_or_default()
            });
        if associations.len() > MAX_KEY_VALUE_STORE_ASSOCIATIONS {
            return Some(format!(
                "the configuration associates {} key value stores with the function but at \
                 most {} may be associated",
                associations.len(),
                MAX_KEY_VALUE_STORE_ASSOCIATIONS
            ));
        }
        for association in associations {
            if self
                .key_value_store_by_arn(&association.key_value_store_arn)
                .is_none()
            {
                return Some(format!(
                    "the key value store {} does not exist",
                    association.key_value_store_arn
                ));
            }
        }

        None
    }

    /// Replace the code and the configuration of the `DEVELOPMENT` stage of the function
    /// `name` names, and report the record as it now stands — or `None` when the store
    /// holds no such function.
    ///
    /// "You can update a function's code or the comment that describes the function. You
    /// cannot update a function's name", and an update addresses no stage: the function's
    /// identity (its name, its ARN and its tags) and its `LIVE` stage are left exactly as
    /// they stand, because `LIVE` holds what a `PublishFunction` COPIED into it. The
    /// stage's `LastModifiedTime` moves to `now` and a FRESH version token is minted, so
    /// the version this caller just replaced no longer satisfies an `If-Match`.
    pub fn apply_function_update(
        &mut self,
        name: &str,
        config: FunctionConfig,
        code: Vec<u8>,
        now: SystemTime,
    ) -> Option<&StoredFunction> {
        if !self.functions.contains_key(name) {
            return None;
        }
        let etag = self.next_etag();
        let stored = self.functions.get_mut(name)?;
        stored.development.config = config;
        stored.development.code = code;
        stored.development.last_modified_time = now;
        stored.development.etag = etag;
        Some(stored)
    }

    /// Whether any distribution this store holds attaches the function `arn`.
    ///
    /// A function is attached to a distribution by a cache behaviour's
    /// `FunctionAssociation$FunctionARN`, so the question is answered against the STORED
    /// distribution configurations rather than against any request: it is what decides
    /// whether a published stage reports [`FUNCTION_STATUS_ASSOCIATED`] or
    /// [`FUNCTION_STATUS_UNASSOCIATED`].
    pub fn function_is_associated(&self, arn: &str) -> bool {
        self.distributions
            .values()
            .any(|distribution| associated_function_arns(&distribution.config).any(|it| it == arn))
    }

    /// Copy the `DEVELOPMENT` stage of the function `name` names into its `LIVE` stage, and
    /// report the summary of the stage that was written — or `None` when this store holds no
    /// function under that name, checked BEFORE the mint so a version token is never spent
    /// on a resource that is not there.
    ///
    /// A publish is a COPY — "copying the function code from the DEVELOPMENT stage to LIVE":
    /// the stage it writes carries the code and the configuration the `DEVELOPMENT` stage
    /// holds at this instant, keeps the created time of the function the copy was taken from,
    /// is last modified now, and gets a FRESH version token of its own, so an `If-Match`
    /// against the `LIVE` stage from before the publish no longer matches. The `DEVELOPMENT`
    /// stage is left exactly as it was — a publish does not edit the stage it reads, and the
    /// operation hands back no new token for it.
    ///
    /// The status the written stage reports is computed from the stored distributions
    /// ([`Self::function_is_associated`]): republishing a function a distribution already
    /// names reports it as associated, publishing one nothing names reports it as
    /// unassociated.
    pub fn publish_function(&mut self, name: &str, now: SystemTime) -> Option<FunctionSummary> {
        let arn = self.functions.get(name)?.arn.clone();
        let status = if self.function_is_associated(&arn) {
            FUNCTION_STATUS_ASSOCIATED
        } else {
            FUNCTION_STATUS_UNASSOCIATED
        };
        let etag = self.next_etag();
        let function = self.functions.get_mut(name)?;
        let published = StoredFunctionStage {
            stage: FUNCTION_STAGE_LIVE.to_string(),
            config: function.development.config.clone(),
            code: function.development.code.clone(),
            status: status.to_string(),
            etag,
            created_time: function.development.created_time,
            last_modified_time: now,
        };
        let summary = published.to_summary(&function.name, &function.arn);
        function.live = Some(published);
        Some(summary)
    }
}

/// Every function ARN the cache behaviours of `config` attach — the default cache behaviour
/// first, then each ordered one.
///
/// A behaviour carries its functions in a LIST wrapper (`FunctionAssociations`), so each
/// behaviour contributes zero or more ARNs rather than the single value
/// [`super::behavior_attachments`] walks; `DefaultCacheBehavior` and `CacheBehavior` are
/// different shapes declaring the same member, so each is read by its own accessor and the
/// walk over them is stated once here.
fn associated_function_arns(config: &DistributionConfig) -> impl Iterator<Item = &str> + '_ {
    std::iter::once(config.default_cache_behavior.function_associations.as_ref())
        .chain(
            config
                .cache_behaviors
                .as_ref()
                .and_then(|behaviors| behaviors.items.as_deref())
                .unwrap_or_default()
                .iter()
                .map(|behavior| behavior.function_associations.as_ref()),
        )
        .flatten()
        .flat_map(|associations| associations.items.as_deref().unwrap_or_default())
        .map(|association| association.function_arn.as_str())
}

/// The ARN of a CloudFront function. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:function/<Name>`. This is the ARN "a
/// distribution's cache behavior" names a published function by, so the resource part is
/// the function's name.
pub fn function_arn(name: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("function/{name}"))
}

/// The path of a function resource under this service's API version. The resource is
/// addressed by NAME, as `GetFunction`'s and `DescribeFunction`'s `{Name}` label is.
pub fn function_path(name: &str) -> String {
    format!("/2020-05-31/function/{name}")
}

/// The modelled `FunctionAlreadyExists` (HTTP 409) for a function name that is already
/// taken, worded as the model documents the shape: "A function with the same name already
/// exists in this Amazon Web Services account. To create a function, you must provide a
/// unique name. To update an existing function, use `UpdateFunction`."
pub fn function_already_exists(name: &str) -> crate::ops::types::FunctionAlreadyExists {
    crate::ops::types::FunctionAlreadyExists {
        message: Some(format!(
            "A function with the name {name} already exists in this Amazon Web Services \
             account. To create a function, you must provide a unique name. To update an \
             existing function, use UpdateFunction."
        )),
    }
}

/// The modelled `NoSuchFunctionExists` (HTTP 404) for a function stage no stored function
/// has — either no function of that name, or a function that has never been published to
/// the stage the request names.
///
/// The sentence is the one the model states on the shape — "The function does not exist."
/// — followed by which function and stage was asked for, which is built from the request
/// and is what tells a caller whether it addressed the wrong name or the wrong stage.
pub fn no_such_function_exists(name: &str, stage: &str) -> NoSuchFunctionExists {
    NoSuchFunctionExists {
        message: Some(format!(
            "The function does not exist: no function named {name} in the {stage} stage"
        )),
    }
}

/// The message for a function configuration whose `KeyValueStoreAssociations.Quantity`
/// disagrees with the length of its `Items`, or `None` when the two agree.
///
/// The SAME rule the parent module's `config_quantity_mismatch` and
/// `response_headers_policy_quantity_mismatch` apply to the configurations they walk:
/// every CloudFront list travels in a wrapper stating its own length beside the list
/// itself, and the service refuses a request where the two disagree — through the parent's
/// one `quantity_disagreement` sentence. `FunctionConfig` reaches exactly one such
/// wrapper. `CreateFunction` models no `InconsistentQuantities`, so its caller reports
/// this reason as the `InvalidArgument` it does model.
pub fn function_config_quantity_mismatch(config: &FunctionConfig) -> Option<String> {
    let associations = config.key_value_store_associations.as_ref()?;
    quantity_disagreement(
        "KeyValueStoreAssociations",
        associations.quantity,
        items_len(&associations.items),
    )
}
