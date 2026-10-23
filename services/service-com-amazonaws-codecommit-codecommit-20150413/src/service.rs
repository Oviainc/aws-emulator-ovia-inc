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

pub mod git_http;
pub mod git_pack;

/// The first label of the public CodeCommit git host
/// (`git-codecommit.<region>.amazonaws.com`), which is what the framework
/// resolves as a route key for a client that was handed that host and had it
/// pointed at this emulator. It names no emulated service — CodeCommit's own
/// signing name is `codecommit` — so a git request carrying it is this service's
/// to claim.
const GIT_HOST_LABEL: &str = "git-codecommit";

/// Run `work` without occupying an async worker.
///
/// The git transport's work is CPU-bound and holds this service's store lock, and
/// it is driven from a SYNCHRONOUS hook (`override_routing_error`), so it cannot
/// be handed to `spawn_blocking(..).await` the way an async handler elsewhere in
/// this workspace hands its interpreter work. `block_in_place` is the construct
/// for that case: it moves the current task off the worker thread so the runtime
/// keeps serving every other request while this one runs. It is available only on
/// a multi-thread runtime — the one the emulator boots — so on a current-thread
/// runtime (a `#[tokio::test]`, or a caller driving the router in-process) the work
/// runs inline, where there is no other worker to protect.
fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

/// The `com-amazonaws-codecommit-codecommit-20150413` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same shared engine the generated entry constructs each operation from.
    ///
    /// The git smart-HTTP transport ([`git_http`]) is not a modeled operation —
    /// git's wire protocol is not in the Smithy model at all — so it is served
    /// from `override_routing_error` below, which is handed the request but no
    /// operation. It reaches this service's store through this handle, exactly as
    /// a handler reaches it through its own `self.engine`.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
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
        (
            "approvalRuleTemplateName" | "oldApprovalRuleTemplateName" | "newApprovalRuleTemplateName",
            _,
        ) if missing => (
            "ApprovalRuleTemplateNameRequiredException",
            "An approval rule template name is required, but was not specified.",
        ),
        (
            "approvalRuleTemplateName" | "oldApprovalRuleTemplateName" | "newApprovalRuleTemplateName",
            ViolationKind::MinLength(_),
        ) => (
            // A name that is present but empty names no template either, so it is
            // the same failure as not specifying one.
            "ApprovalRuleTemplateNameRequiredException",
            "An approval rule template name is required, but was not specified.",
        ),
        (
            "approvalRuleTemplateName" | "oldApprovalRuleTemplateName" | "newApprovalRuleTemplateName",
            _,
        ) => (
            "InvalidApprovalRuleTemplateNameException",
            "The name of the approval rule template is not valid. Template names must be between 1 and 100 valid characters in length.",
        ),
        // `newRuleContent` is deliberately NOT listed: UpdatePullRequestApprovalRuleContent
        // spells its member that way too, and that operation models
        // `ApprovalRuleContentRequiredException` / `InvalidApprovalRuleContentException`
        // instead, so one name-keyed row cannot speak for both.
        ("approvalRuleTemplateContent", _) if missing => (
            "ApprovalRuleTemplateContentRequiredException",
            "The content for the approval rule template is empty. You must provide some content for an approval rule template. The content cannot be null.",
        ),
        ("approvalRuleTemplateContent", ViolationKind::MinLength(_)) => (
            "ApprovalRuleTemplateContentRequiredException",
            "The content for the approval rule template is empty. You must provide some content for an approval rule template. The content cannot be null.",
        ),
        ("approvalRuleTemplateContent", _) => (
            "InvalidApprovalRuleTemplateContentException",
            "The content of the approval rule template is not valid.",
        ),
        ("approvalRuleTemplateDescription", _) => (
            "InvalidApprovalRuleTemplateDescriptionException",
            "The description for the approval rule template is not valid because it exceeds the maximum characters allowed for a description.",
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
        let route_key = smithy_server_runtime::resolve_route_key(parts);
        if route_key
            .as_deref()
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request to the git smart-HTTP transport, which is claimed by PATH
        // because that is the only thing that identifies it: a git client sends no
        // SigV4 credential and no `X-Amz-Target`, and addresses whatever host the
        // clone URL named — the emulator's own endpoint (an IP literal or
        // `localhost`, which name no service) or the public CodeCommit git host,
        // whose label names no service either. Without this the router answers
        // `UnknownService` for a URL this service handed out.
        //
        // A request that DOES name another service is left to it: a second
        // claimant would only make the router refuse both as `AmbiguousService`.
        if route_key
            .as_deref()
            .is_some_and(|key| key != GIT_HOST_LABEL)
        {
            return false;
        }
        // `http::Uri::path` already strips the scheme, the authority and the
        // query, for an origin-form request line and an absolute-form one alike.
        git_http::route(parts.method.as_str(), parts.uri.path(), parts.uri.query()).is_some()
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A git smart-HTTP request is exactly such a request and is served here:
        // git's transport is not in the Smithy model, so it names no modeled
        // operation and no routing key, and it carries no SigV4 signature for the
        // framework to verify (a git client authenticates with HTTPS git
        // credentials, which this emulator does not mint). It is answered in git's
        // own wire protocol rather than in an AWS error envelope, because a git
        // client is what reads it.
        let (path, query) = git_http::split_uri(req.uri());
        let request = git_http::route(req.method(), &path, query.as_deref())?;
        // A body that is not in memory is a body this service never read: it must
        // not be served as an EMPTY one, which a push would answer as a no-op
        // success. The router buffers every request body, so this is the
        // can't-happen arm, refused the same way an unreachable store is.
        let unreadable = || git_http::Response {
            status: 500,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: b"the request body could not be read\n".to_vec(),
        };
        let response = match (
            crate::state::service_state(&*self.engine),
            req.body().bytes(),
        ) {
            (Some(state), Some(body)) => {
                let content_encoding = req.headers().get("content-encoding");
                // OFF THE ASYNC WORKER. Reading a pack (inflate, delta
                // resolution) and projecting a push are CPU-bound, and they hold
                // this service's store lock for their whole span, so they must not
                // occupy a runtime worker: `block_in_place` hands the current task
                // its own thread and lets the runtime keep serving every other
                // request while this one runs. This hook is synchronous — the
                // vendored router calls it from `dispatch` — so there is no
                // `spawn_blocking(..).await` to use instead; `block_in_place` is
                // the construct for exactly that case. It exists only on a
                // multi-thread runtime, which is the one the emulator boots, so a
                // current-thread runtime (a `#[tokio::test]`, say) runs the work
                // inline, where there is no other worker to protect.
                off_the_async_worker(|| {
                    git_http::serve(&mut state.lock(), &request, content_encoding, body)
                })
            }
            (None, _) => git_http::Response {
                status: 500,
                content_type: "text/plain; charset=utf-8".to_string(),
                body: b"the repository store is unavailable\n".to_vec(),
            },
            (_, None) => unreadable(),
        };
        let mut rendered = aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(response.status)
                .unwrap_or_else(|_| aws_smithy_runtime_api::http::StatusCode::try_from(500u16).expect("500 is a status")),
            aws_smithy_types::body::SdkBody::from(response.body),
        );
        rendered
            .headers_mut()
            .insert("content-type", response.content_type);
        // A git client must never serve a ref advertisement or a packfile from a
        // cache: both are a snapshot of refs that the next request may move.
        rendered.headers_mut().insert("cache-control", "no-cache");
        rendered.headers_mut().insert("pragma", "no-cache");
        Some(rendered)
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
