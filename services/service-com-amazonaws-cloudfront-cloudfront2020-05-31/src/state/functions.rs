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
    FunctionConfig, FunctionMetadata, FunctionSummary, NoSuchFunctionExists,
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
