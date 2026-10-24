// SPDX-License-Identifier: CC0-1.0

//! The CONNECTION FUNCTION slice of this service's state.
//!
//! A connection function is its own resource kind — addressed by a minted `Id` rather than
//! by its name, and projected as a `ConnectionFunctionSummary` rather than a
//! `FunctionSummary` — so everything about how one is persisted and projected lives here
//! rather than in the parent module: the record each stage carries, the ARN and the path a
//! connection function is addressed by, the lookups every read goes through, and the
//! modelled errors the connection function operations answer with. The parent
//! [`ComAmazonawsCloudfrontCloudfront20200531State`] holds the one map
//! (`connection_functions`) and this module holds everything that reads or writes it.
//!
//! The STAGE vocabulary is shared with the CloudFront function surface rather than
//! duplicated: `ConnectionFunctionSummary$Stage` targets the same modelled `FunctionStage`
//! enum (`DEVELOPMENT` | `LIVE`) a `FunctionMetadata$Stage` does, so
//! [`super::FUNCTION_STAGE_DEVELOPMENT`], [`super::FUNCTION_STAGE_LIVE`],
//! [`super::DEFAULT_FUNCTION_STAGE`] and [`super::FUNCTION_STATUS_UNPUBLISHED`] are the
//! values named here too.
//!
//! Nothing in here is owed by the CLOCK: a connection function this service accepts exists
//! in `DEVELOPMENT` immediately and reaches `LIVE` only through a
//! `PublishConnectionFunction` CALL, so no status here is transient and the parent's
//! `settle` has nothing to do for one.

use std::time::SystemTime;

use crate::ops::types::{
    ConnectionFunctionSummary, EntityAlreadyExists, EntityNotFound, FunctionConfig,
};

use super::{
    ComAmazonawsCloudfrontCloudfront20200531State, TagSet, ID_BODY_LEN, MAX_FUNCTIONS,
    MAX_FUNCTION_CODE_BYTES,
};

/// The prefix every connection function id this service mints carries.
///
/// `DescribeConnectionFunction`'s `Identifier` is "The connection function's identifier",
/// which the three namespaces a connection function answers to share (its id, its name and
/// its ARN), so a minted id is given a prefix of its own for the same reason
/// [`super::DISTRIBUTION_TENANT_ID_PREFIX`] and [`super::CONNECTION_GROUP_ID_PREFIX`] have
/// one: the model states no id pattern, so the shape is this emulator's choice, and a
/// resolution order ([`ComAmazonawsCloudfrontCloudfront20200531State::connection_function`])
/// decides the rest.
pub const CONNECTION_FUNCTION_ID_PREFIX: &str = "cf_";

/// The most connection functions this emulator holds, refused at with the modelled
/// `EntityLimitExceeded` ("The entity limit has been exceeded").
///
/// CloudFront publishes a quota of 100 "Functions per Amazon Web Services account"
/// ([`MAX_FUNCTIONS`]) and states no separate number for a connection function, so the
/// bound is that same count rather than a second one invented here. It is an entry count
/// checked before anything is minted: a connection function record is what a later read
/// answers from, so it cannot be evicted.
pub const MAX_CONNECTION_FUNCTIONS: usize = MAX_FUNCTIONS;

/// The largest connection function code this emulator accepts, refused at with the
/// modelled `EntitySizeLimitExceeded` ("The entity size limit was exceeded").
///
/// The code travels as the same `FunctionBlob` (up to 40960 bytes on the wire) a CloudFront
/// function's does, and CloudFront's published quota for the "Maximum size of the function
/// code and included libraries" is 10 KB ([`MAX_FUNCTION_CODE_BYTES`]) — which is exactly
/// what makes this refusal reachable: the boundary validator rejects anything past the
/// modelled bound, and this quota rejects code that is within the model but past what
/// CloudFront runs.
pub const MAX_CONNECTION_FUNCTION_CODE_BYTES: usize = MAX_FUNCTION_CODE_BYTES;

/// One stage of one connection function, as this emulator persists it.
///
/// A connection function's two stages — `DEVELOPMENT` and `LIVE` — each carry their OWN
/// code, configuration, status and version token, as a CloudFront function's do: a publish
/// COPIES one stage into the other, so an edit to one stage is not an edit to the other and
/// each stage answers reads with what was last copied or written into it.
#[derive(Debug, Clone)]
pub struct StoredConnectionFunctionStage {
    /// Which stage this record is: [`super::FUNCTION_STAGE_DEVELOPMENT`] or
    /// [`super::FUNCTION_STAGE_LIVE`], reported as `ConnectionFunctionSummary$Stage`.
    pub stage: String,
    /// The configuration as submitted ("Contains configuration information about a
    /// CloudFront function"). Stored whole, so a read reports what was stored rather than
    /// re-deriving anything from a request.
    pub config: FunctionConfig,
    /// The connection function code as submitted, which `GetConnectionFunction` answers
    /// with byte for byte.
    pub code: Vec<u8>,
    /// `ConnectionFunctionSummary$Status` — "The connection function status."
    pub status: String,
    /// The version token the last mutation of this stage handed back, and the single source
    /// of truth for every `If-Match` check against it.
    pub etag: String,
    /// `ConnectionFunctionSummary$CreatedTime` — "The connection function created time."
    pub created_time: SystemTime,
    /// `ConnectionFunctionSummary$LastModifiedTime` — "The connection function last
    /// modified time."
    pub last_modified_time: SystemTime,
}

impl StoredConnectionFunctionStage {
    /// The `ConnectionFunctionSummary` projection every connection function operation
    /// returns for this stage, read from the one stored record.
    ///
    /// The name, the id and the ARN belong to the connection function rather than to one
    /// stage, so they are passed in by [`StoredConnectionFunction`]: both stages of one
    /// connection function report the SAME id and ARN.
    pub fn to_summary(&self, name: &str, id: &str, arn: &str) -> ConnectionFunctionSummary {
        ConnectionFunctionSummary {
            name: name.to_string(),
            id: id.to_string(),
            connection_function_config: self.config.clone(),
            connection_function_arn: arn.to_string(),
            status: self.status.clone(),
            stage: self.stage.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
        }
    }
}

/// One connection function, as this emulator persists it.
///
/// The record is the single source of truth for the id and the ARN minted for it, the name
/// it was created under, each stage's code, configuration, status, timestamps and version
/// token, and the tags it carries — so the create and every later read answer from it
/// rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredConnectionFunction {
    /// `ConnectionFunctionSummary$Id` — "The connection function ID", minted at creation
    /// and the key this record is held under: it is what `DeleteConnectionFunction`,
    /// `UpdateConnectionFunction` and `PublishConnectionFunction` address the resource by.
    pub id: String,
    /// `ConnectionFunctionSummary$Name` — "The connection function name", unique per
    /// account, which is what makes a repeated name an `EntityAlreadyExists`.
    pub name: String,
    /// `ConnectionFunctionSummary$ConnectionFunctionArn` — "The connection function Amazon
    /// Resource Name (ARN)". Minted at creation and the same for both stages.
    pub arn: String,
    /// The `DEVELOPMENT` stage, which exists for the whole life of the connection function:
    /// a create puts the code and the configuration it was given into it.
    pub development: StoredConnectionFunctionStage,
    /// The `LIVE` stage, which exists only once `PublishConnectionFunction` has copied the
    /// `DEVELOPMENT` stage into it — `None` until then, which is what makes a read of the
    /// `LIVE` stage of a never-published connection function an entity that is not found.
    pub live: Option<StoredConnectionFunctionStage>,
    /// The tags submitted with the create, in the SAME representation every tagged resource
    /// of this service uses, so one tagging rule covers them all.
    pub tags: TagSet,
}

impl StoredConnectionFunction {
    /// The stage `stage` names, or `None` when this connection function has no such stage —
    /// either because it has never been published, or because the value names no stage at
    /// all.
    ///
    /// The one lookup every connection function read goes through: the
    /// `ConnectionFunctionSummary` and the version token a read reports both come from the
    /// stage record it answers.
    pub fn stage(&self, stage: &str) -> Option<&StoredConnectionFunctionStage> {
        match stage {
            super::FUNCTION_STAGE_DEVELOPMENT => Some(&self.development),
            super::FUNCTION_STAGE_LIVE => self.live.as_ref(),
            _ => None,
        }
    }
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// The connection function `identifier` names, or `None` when the store holds none.
    ///
    /// `DescribeConnectionFunction` and `GetConnectionFunction` name their resource by "The
    /// connection function's identifier", which it answers to by id, by name or by ARN, so
    /// all three resolve to the one record: the id is the store's key, and the name and the
    /// ARN are compared against the STORED values rather than re-derived out of the
    /// request. The id is tried FIRST, so a minted id always names the record it was minted
    /// for.
    pub fn connection_function(&self, identifier: &str) -> Option<&StoredConnectionFunction> {
        if let Some(function) = self.connection_functions.get(identifier) {
            return Some(function);
        }
        self.connection_functions
            .values()
            .find(|function| function.name == identifier || function.arn == identifier)
    }

    /// The connection function created under `name`, or `None` when the store holds none.
    ///
    /// `CreateConnectionFunction` models `EntityAlreadyExists` ("The entity already exists.
    /// You must provide a unique entity"), so a name names at most ONE connection function
    /// and this is what makes a repeated name a conflict rather than a second resource.
    pub fn connection_function_named(&self, name: &str) -> Option<&StoredConnectionFunction> {
        self.connection_functions
            .values()
            .find(|function| function.name == name)
    }

    /// The connection function `arn` names, or `None` when this store holds none.
    ///
    /// The ARN a connection function carries is the one minted when it was created, so the
    /// lookup compares against the STORED value rather than re-deriving an identifier out
    /// of the request. ONE ARN lookup for this resource kind, which the tag read goes
    /// through.
    pub fn connection_function_by_arn(&self, arn: &str) -> Option<&StoredConnectionFunction> {
        self.connection_functions
            .values()
            .find(|function| function.arn == arn)
    }

    /// Allocate a connection function id no stored connection function holds:
    /// [`CONNECTION_FUNCTION_ID_PREFIX`] followed by a minted token, drawn from the same
    /// mint as every other id this service generates so no two are minted the same.
    pub fn allocate_connection_function_id(&mut self) -> String {
        loop {
            let candidate = format!("{CONNECTION_FUNCTION_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.connection_functions.contains_key(&candidate) {
                break candidate;
            }
        }
    }
}

/// The ARN of a connection function. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:connection-function/<Id>`. The resource
/// part is the minted id, which is the identifier every mutating connection function
/// operation addresses one by.
pub fn connection_function_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("connection-function/{id}"))
}

/// The path of a connection function resource under this service's API version. The
/// resource is addressed by ID, as `GetConnectionFunction`'s `{Identifier}` label and
/// `DeleteConnectionFunction`'s `{Id}` label are.
pub fn connection_function_path(id: &str) -> String {
    format!("/2020-05-31/connection-function/{id}")
}

/// The modelled `EntityNotFound` (HTTP 404) for a connection function stage no stored
/// connection function has — either no connection function answering to that identifier, or
/// one that has never been published to the stage the request names.
///
/// The sentence is the one the model states on the shape — "The entity was not found." —
/// followed by which identifier and stage was asked for, which is built from the request and
/// is what tells a caller whether it addressed the wrong resource or the wrong stage.
pub fn connection_function_not_found(identifier: &str, stage: &str) -> EntityNotFound {
    EntityNotFound {
        message: Some(format!(
            "The entity was not found: no connection function {identifier} in the {stage} stage"
        )),
    }
}

/// The modelled `EntityAlreadyExists` (HTTP 409) for a connection function name that is
/// already taken, worded as the model documents the shape: "The entity already exists. You
/// must provide a unique entity."
pub fn connection_function_already_exists(name: &str) -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(format!(
            "The entity already exists. You must provide a unique entity: a connection \
             function with the name {name} already exists in this Amazon Web Services account"
        )),
    }
}
