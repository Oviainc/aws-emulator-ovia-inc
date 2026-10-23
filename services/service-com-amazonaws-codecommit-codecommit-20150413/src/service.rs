// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codecommit-codecommit-20150413` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The `com-amazonaws-codecommit-codecommit-20150413` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The error CodeCommit names for one rejected member, or `None` to leave the
/// framework's wording in place.
///
/// Each code is an error shape this service's model declares, and each message is
/// that shape's own documented sentence (the `//! ## Errors` list in
/// `src/ops/<operation>.rs`), followed by the member at fault so the caller can
/// fix the request from the message alone.
fn refusal_for(violation: &Violation) -> Option<Refused> {
    // A nested path names its top-level member first (`tags.aws:x.member`), and a
    // member of a nested shape is its LAST segment
    // (`location.relativeFileVersion`); CodeCommit names the error for the member
    // actually at fault, so the table below is keyed on the top-level member and,
    // for a nested shape's own member, on that leaf.
    let member = violation.path.split('.').next().unwrap_or_default();
    let leaf = violation.path.rsplit('.').next().unwrap_or_default();
    let missing = matches!(violation.kind, ViolationKind::MissingRequiredMember);
    let (code, requirement) = match (member, &violation.kind) {
        ("repositoryName", _) if missing => (
            "RepositoryNameRequiredException",
            "A repository name is required, but was not specified.",
        ),
        ("repositoryName", _) => (
            "InvalidRepositoryNameException",
            "A specified repository name is not valid.",
        ),
        ("repositoryNames", _) if missing => (
            "RepositoryNamesRequiredException",
            "A repository names object is required, but was not specified.",
        ),
        ("repositoryDescription", _) => (
            "InvalidRepositoryDescriptionException",
            "The specified repository description is not valid.",
        ),
        // Every member that carries a `BranchName` names a branch, so each takes the
        // branch errors: `defaultBranchName` is `UpdateDefaultBranch`'s name for one.
        ("branchName" | "defaultBranchName", _) if missing => (
            "BranchNameRequiredException",
            "A branch name is required, but was not specified.",
        ),
        ("branchName" | "defaultBranchName", _) => (
            "InvalidBranchNameException",
            "The specified reference name is not valid.",
        ),
        ("filePath" | "folderPath", _) if missing => (
            "PathRequiredException",
            "The folderPath for a location cannot be null.",
        ),
        ("filePath" | "folderPath", _) => {
            ("InvalidPathException", "The specified path is not valid.")
        }
        ("fileContent", _) if missing => (
            "FileContentRequiredException",
            "The file cannot be added because it is empty.",
        ),
        ("fileContent", ViolationKind::MaxLength(_)) => (
            "FileContentSizeLimitExceededException",
            "The file cannot be added because it is too large.",
        ),
        ("fileMode", ViolationKind::EnumValue(_)) => (
            "InvalidFileModeException",
            "The specified file mode permission is not valid.",
        ),
        ("parentCommitId", _) if missing => (
            "ParentCommitIdRequiredException",
            "A parent commit ID is required.",
        ),
        ("parentCommitId", _) => (
            "InvalidParentCommitIdException",
            "The parent commit ID is not valid.",
        ),
        ("commitId" | "afterCommitId" | "beforeCommitId", _) if missing => (
            "CommitIdRequiredException",
            "A commit ID was not specified.",
        ),
        ("commitId" | "afterCommitId" | "beforeCommitId", _) => {
            ("InvalidCommitIdException", "The specified commit ID is not valid.")
        }
        ("commentId", _) if missing => (
            "CommentIdRequiredException",
            "The comment ID is missing or null. A comment ID is required.",
        ),
        ("commentId", _) => (
            "InvalidCommentIdException",
            "The comment ID is not in a valid format.",
        ),
        ("reactionValue", _) if missing => (
            "ReactionValueRequiredException",
            "A reaction value is required.",
        ),
        ("blobId", _) if missing => (
            "BlobIdRequiredException",
            "A blob ID is required, but was not specified.",
        ),
        ("blobId", _) => ("InvalidBlobIdException", "The specified blob is not valid."),
        ("commitMessage", ViolationKind::MaxLength(_)) => (
            "CommitMessageLengthExceededException",
            "The commit message is too long.",
        ),
        ("name", ViolationKind::MaxLength(_)) => (
            "NameLengthExceededException",
            "The user name is not valid because it has exceeded the character limit for author names.",
        ),
        ("email", _) => (
            "InvalidEmailException",
            "The specified email address either contains one or more characters that are not allowed, or it exceeds the maximum number of characters allowed for an email address.",
        ),
        ("content", _) if missing => (
            "CommentContentRequiredException",
            "The comment is empty. You must provide some content for a comment.",
        ),
        ("content", ViolationKind::MaxLength(_)) => (
            "CommentContentSizeLimitExceededException",
            "The comment is too large. Comments are limited to 10,240 characters.",
        ),
        // `relativeFileVersion` is a member of the nested `Location` shape, so it
        // is the path's LEAF rather than its top-level member.
        (_, ViolationKind::EnumValue(_)) if leaf == "relativeFileVersion" => (
            "InvalidRelativeFileVersionEnumException",
            "Either the enum is not in a valid format, or the specified file version enum is not valid in respect to the current file version.",
        ),
        ("clientRequestToken", _) if missing => (
            "ClientRequestTokenRequiredException",
            "A client request token is required.",
        ),
        ("clientRequestToken", _) => (
            "InvalidClientRequestTokenException",
            "The client request token is not valid.",
        ),
        ("sortBy", ViolationKind::EnumValue(_)) => (
            "InvalidSortByException",
            "The specified sort by value is not valid.",
        ),
        ("order", ViolationKind::EnumValue(_)) => (
            "InvalidOrderException",
            "The specified sort order is not valid.",
        ),
        ("nextToken", _) => (
            "InvalidContinuationTokenException",
            "The specified continuation token is not valid.",
        ),
        ("tags", _) if missing => (
            "TagsMapRequiredException",
            "A map of tags is required.",
        ),
        ("tags", _) => ("InvalidTagsMapException", "The map of tags is not valid."),
        ("tagKeys", _) if missing => (
            "TagKeysListRequiredException",
            "A list of tag keys is required. The list cannot be empty or null.",
        ),
        ("tagKeys", _) => (
            "InvalidTagKeysListException",
            "The list of tags is not valid.",
        ),
        ("resourceArn", _) if missing => (
            "ResourceArnRequiredException",
            "A valid Amazon Resource Name (ARN) for an CodeCommit resource is required.",
        ),
        ("resourceArn", _) => (
            "InvalidResourceArnException",
            "The value for the resource ARN is not valid.",
        ),
        // Everything else keeps the framework's own wording.
        _ => return None,
    };
    Some(Refused {
        code: code.to_string(),
        message: format!("{requirement} (member: {})", violation.path),
    })
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // CodeCommit does not answer one generic `ValidationException`: it names a
        // specific error per member — a missing repository name is
        // `RepositoryNameRequiredException`, a `sortBy` outside its enum is
        // `InvalidSortByException`, and so on. A client branches on that code, so the
        // framework's refusal is reworded here (code AND message) into the error the
        // model declares for the member at fault. The HTTP status stays with the
        // protocol, and every error named below is a 400 in this model.
        match refusal {
            Refusal::Invalid(violations) => violations.iter().find_map(refusal_for),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
