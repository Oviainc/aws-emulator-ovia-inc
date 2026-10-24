// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codecommit-codecommit-20150413 service.
//!
//! The store is a map of live repositories, each carrying its own git object
//! store, branches, tags, the commit projection the control plane reads and the
//! comments posted on its commit comparisons, so every response an operation
//! returns is a lookup in state rather than a value made up on the request path.
//!
//! A repository has exactly ONE object store ([`Repository::objects`]), keyed by
//! git object name, and it is what both faces of the service read: the control
//! plane projects it into the modeled shapes (`GetFile`, `GetBranch`), and the git
//! smart-HTTP transport in [`crate::service::git_http`] serves and receives the
//! very same objects. Object ids are therefore real git object names — see
//! [`git_object`] for the name function — so a `git clone` of content written by
//! `PutFile` resolves, and a `GetFile` of content written by `git push` resolves.
//!
//! The [`Commit`] records in [`Repository::commits`] are a PROJECTION of the
//! commit objects: each carries the flattened tree snapshot at that commit, so
//! reading a file at a commit specifier is a lookup rather than a tree walk.

pub mod git_object;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{ApprovalRuleTemplate, Comment, Location, RepositoryMetadata, UserInfo};
use git_object::{Kind, Object, TreeEntry};

/// The region an ARN or clone URL uses when the request carries no SigV4
/// credential scope to read one from (an unsigned request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `relativeFileVersion` values the model's `RelativeFileVersionEnum` declares
/// (`smithy_query(query="shape",shape="RelativeFileVersionEnum")`): whether the
/// commented version of a file is the before or the after of the comparison.
pub const RELATIVE_FILE_VERSION_BEFORE: &str = "BEFORE";
pub const RELATIVE_FILE_VERSION_AFTER: &str = "AFTER";

/// The largest comment CodeCommit accepts, in characters. Documented on
/// `CommentContentSizeLimitExceededException`: "The comment is too large. Comments
/// are limited to 10,240 characters."
pub const MAX_COMMENT_CHARACTERS: usize = 10_240;

/// How many comment threads one `GetCommentsForComparedCommit` page carries when
/// the caller names no `maxResults`, and the largest value that member accepts.
/// Documented on it: "A non-zero, non-negative integer used to limit the number of
/// returned results. The default is 100 comments, but you can configure up to
/// 500."
pub const COMMENTS_PAGE_DEFAULT: i32 = 100;
pub const COMMENTS_PAGE_MAX: i32 = 500;

/// How many comments one repository holds before `PostCommentForComparedCommit`
/// refuses the write.
///
/// The emulator's OWN bound, not an AWS limit: a comment is caller-supplied text
/// kept for the life of the repository, so this is what keeps one repository's
/// stored size bounded. Every record it holds is released by `DeleteRepository`.
pub const MAX_COMMENTS_PER_REPOSITORY: usize = 1_000;

/// `fileMode` values the model's `FileModeTypeEnum` declares
/// (`smithy_query(query="shape",shape="FileModeTypeEnum")`).
pub const FILE_MODE_NORMAL: &str = "NORMAL";
pub const FILE_MODE_EXECUTABLE: &str = "EXECUTABLE";
pub const FILE_MODE_SYMLINK: &str = "SYMLINK";

/// Upper bound on the number of segments in a file path. Not an AWS limit: it
/// bounds the recursion in [`tree_objects`], which walks one level per path
/// segment, so a pathological path cannot exhaust the stack. Enforced in BOTH
/// places a path's depth can drive that walk: [`normalize_path`] refuses a deeper
/// `filePath` at the request boundary, and `build_tree` (under [`tree_objects`])
/// refuses a deeper snapshot however it was assembled — a pushed packfile reaches
/// the builder without passing a `filePath` through [`normalize_path`].
pub const MAX_PATH_SEGMENTS: usize = 256;

/// How many repositories the store holds before `CreateRepository` refuses with
/// the modelled `RepositoryLimitExceededException`.
///
/// CodeCommit publishes an adjustable quota on repositories per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is far above any workflow an emulator
/// serves, and it is what stops the repository map growing without end; every
/// record it holds is released by `DeleteRepository`.
pub const REPOSITORY_QUOTA: usize = 1_000;

/// How many files one repository's tree holds before `PutFile` refuses the write
/// and before the git transport refuses a push whose commit carries a larger
/// tree.
///
/// The emulator's OWN bound, not an AWS limit: a commit's projection stores the
/// whole tree snapshot, so this is what keeps both the per-request work and one
/// repository's stored size bounded.
pub const MAX_FILES_PER_TREE: usize = 1_000;

/// How many commits one repository's PROJECTION retains. Older commits that are
/// not the tip of a ref are dropped by [`Repository::prune_history`], so
/// repeated writes cannot grow the store without end.
///
/// The emulator's OWN bound, not an AWS limit: CodeCommit keeps all history. A
/// read at a commit that has been dropped answers `CommitDoesNotExistException`,
/// the same as a commit id the repository never held. With
/// [`MAX_FILES_PER_TREE`] it bounds the product of commits and files per commit,
/// which is what one repository can cost.
pub const MAX_COMMITS_RETAINED: usize = 256;

/// How many git objects one repository's object store holds. The emulator's OWN
/// bound, not an AWS limit: it is what stops a push growing the store without
/// end, and a push that would pass it is refused by the transport.
pub const MAX_OBJECTS_PER_REPOSITORY: usize = 20_000;

/// How many bytes of git object payload one repository's object store holds. The
/// emulator's OWN bound, for the same reason as [`MAX_OBJECTS_PER_REPOSITORY`].
pub const MAX_OBJECT_BYTES_PER_REPOSITORY: usize = 64 * 1024 * 1024;

/// How many branches one repository holds before `CreateBranch`, or a push
/// through the git transport, refuses the write.
///
/// The emulator's OWN bound, not an AWS limit: a branch is a caller-named
/// reference kept for the life of the repository, and
/// [`Repository::prune_history`] never drops a branch tip, so an unbounded number
/// of branches would pin an unbounded number of commits and defeat
/// [`MAX_COMMITS_RETAINED`]. It is therefore deliberately well BELOW that bound,
/// which is what lets pruning always reach it. Every record it holds is released
/// by `DeleteRepository`.
pub const MAX_BRANCHES_PER_REPOSITORY: usize = 64;

/// How many tags one repository holds before a push refuses to create another.
///
/// The emulator's OWN bound, for exactly the reason
/// [`MAX_BRANCHES_PER_REPOSITORY`] gives and at the same figure: a tag is a
/// caller-named reference kept for the life of the repository and never pruned,
/// so it pins history just as a branch does. CodeCommit has no control-plane API
/// that creates a tag, so the git transport is the only writer.
pub const MAX_TAGS_PER_REPOSITORY: usize = MAX_BRANCHES_PER_REPOSITORY;

/// The largest total content size of one folder, in bytes. CodeCommit documents
/// this limit on `FolderContentSizeLimitExceededException`: a commit cannot be
/// created when a change "results in a folder whose contents exceed the limit of
/// 6 MB".
pub const MAX_FOLDER_CONTENT_BYTES: i64 = 6 * 1024 * 1024;

/// The git ref namespace a branch lives in.
pub const HEADS_PREFIX: &str = "refs/heads/";
/// The git ref namespace a tag lives in.
pub const TAGS_PREFIX: &str = "refs/tags/";

/// How many tree entries one request's projection may walk in total, across
/// every commit it projects.
///
/// The emulator's OWN bound, and the bound that holds where the depth and the
/// file count do not: a git tree is a DAG, so a few tree objects naming one
/// another present exponentially many paths (see
/// [`Repository::flatten_tree`]). It is [`MAX_COMMITS_PROJECTED_PER_REQUEST`]
/// times four times [`MAX_FILES_PER_TREE`] — room for every commit the request
/// may project to carry a full snapshot and its directories — and it is charged
/// per entry, so no product of commands, commits and entries escapes it.
pub const MAX_TREE_ENTRIES_PER_REQUEST: usize =
    MAX_COMMITS_PROJECTED_PER_REQUEST * 4 * MAX_FILES_PER_TREE;

/// How many commits one request's projection may build.
///
/// The emulator's OWN bound: a repository retains at most
/// [`MAX_COMMITS_RETAINED`] projected commits, so projecting more than that in
/// one request would only build snapshots that are pruned on the way out.
pub const MAX_COMMITS_PROJECTED_PER_REQUEST: usize = MAX_COMMITS_RETAINED;

/// How long one request's projection may take before it is refused. The
/// emulator's own bound, at the same figure as the pack reader's deadline and the
/// workspace's other caller-driven CPU work: the budgets above bound the work, and
/// this bounds the time it may take on any machine, so the store lock the
/// projection runs under is never held for an unbounded span. The projection also
/// runs off the async worker — see `crate::service`'s `off_the_async_worker`.
pub const MAX_PROJECTION_SECONDS: u64 = 2;

/// What ONE request's whole projection may still spend: commits, tree entries
/// and time.
///
/// One budget is built per request and threaded through every projection a
/// request drives ([`Repository::project_history`], and so
/// [`Repository::flatten_tree`]), because the cost is a PRODUCT: a push carries
/// many ref updates, each naming a commit whose ancestors each carry a whole tree
/// snapshot. Per-call caps would bound each factor and none of the product.
#[derive(Debug)]
pub struct ProjectionBudget {
    commits_left: usize,
    entries_left: usize,
    deadline: std::time::Instant,
}

impl Default for ProjectionBudget {
    fn default() -> Self {
        Self::for_one_request()
    }
}

impl ProjectionBudget {
    /// The allowance one request gets.
    pub fn for_one_request() -> Self {
        Self {
            commits_left: MAX_COMMITS_PROJECTED_PER_REQUEST,
            entries_left: MAX_TREE_ENTRIES_PER_REQUEST,
            deadline: std::time::Instant::now()
                + std::time::Duration::from_secs(MAX_PROJECTION_SECONDS),
        }
    }

    /// Charge one projected commit.
    fn commit(&mut self) -> Result<(), String> {
        self.check_deadline()?;
        self.commits_left = self.commits_left.checked_sub(1).ok_or_else(|| {
            format!(
                "this request would project more than the {MAX_COMMITS_PROJECTED_PER_REQUEST} commits this emulator projects at once"
            )
        })?;
        Ok(())
    }

    /// Charge one tree entry walked.
    ///
    /// `pub` because the walks a request drives outside this file are charged to
    /// the same budget: a push's per-ref closure check
    /// ([`Repository::first_missing_object_within`]) walks tree entries exactly as
    /// a projection does, and one request may drive it once per ref it updates, so
    /// the SUM of every walk is what this bounds.
    pub fn entry(&mut self) -> Result<(), String> {
        self.entries_left = self.entries_left.checked_sub(1).ok_or_else(|| {
            format!(
                "this request would walk more than the {MAX_TREE_ENTRIES_PER_REQUEST} tree entries this emulator walks at once"
            )
        })?;
        // Checked here as well as per commit: a single commit's tree walk is the
        // one step that can run long on its own.
        if self.entries_left.is_multiple_of(1024) {
            self.check_deadline()?;
        }
        Ok(())
    }

    fn check_deadline(&self) -> Result<(), String> {
        if std::time::Instant::now() > self.deadline {
            return Err(format!(
                "this request took longer than the {MAX_PROJECTION_SECONDS} seconds this emulator spends projecting a push"
            ));
        }
        Ok(())
    }
}

/// How many tags one CodeCommit resource carries before a tagging request is
/// refused with the modelled `TooManyTagsException`.
///
/// AWS's tagging limit for a CodeCommit resource: "You can add up to 50 tags to
/// a single CodeCommit repository" (CodeCommit User Guide, "Tag a repository"),
/// which is also the general AWS limit of 50 user-created tags per resource.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagKey`'s and `TagValue`'s modelled `length` bounds
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`). The boundary
/// validator applies a `length` to a MEMBER, not to the keys and values inside a
/// map or list, so the tagging operations apply these themselves — a request
/// outside them is `InvalidTagsMapException` / `InvalidTagKeysListException`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;
pub const TAG_KEY_MAX_LENGTH: usize = 128;
pub const TAG_VALUE_MIN_LENGTH: usize = 0;
pub const TAG_VALUE_MAX_LENGTH: usize = 256;

/// Is this within `TagKey`'s modelled length? Smithy counts a string's length in
/// Unicode code points.
pub fn is_tag_key(candidate: &str) -> bool {
    (TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&candidate.chars().count())
}

/// Is this within `TagValue`'s modelled length?
pub fn is_tag_value(candidate: &str) -> bool {
    (TAG_VALUE_MIN_LENGTH..=TAG_VALUE_MAX_LENGTH).contains(&candidate.chars().count())
}

/// The tag key prefix AWS reserves for its own system tags. A request that sets
/// or removes a key carrying it is refused with `InvalidSystemTagUsageException`:
/// "The specified tag is not valid. Key names cannot be prefixed with aws:."
pub const SYSTEM_TAG_PREFIX: &str = "aws:";

/// Is this a tag key only AWS itself may set, i.e. one carrying
/// [`SYSTEM_TAG_PREFIX`]?
pub fn is_system_tag_key(key: &str) -> bool {
    key.starts_with(SYSTEM_TAG_PREFIX)
}

/// `RepositoryName`'s modelled `length` bounds
/// (`smithy_query(query="shape",shape="RepositoryName")`).
pub const REPOSITORY_NAME_MIN_LENGTH: usize = 1;
pub const REPOSITORY_NAME_MAX_LENGTH: usize = 100;

/// Does `candidate` satisfy `RepositoryName` — its modelled `length` 1..100 and
/// its modelled `pattern` `^[\w\.-]+$`
/// (`smithy_query(query="shape",shape="RepositoryName")`)? The boundary
/// validator already applies both to a `repositoryName` MEMBER; this is for a
/// name that arrives inside another value, such as the resource portion of a
/// resource ARN.
pub fn is_repository_name(candidate: &str) -> bool {
    let length = candidate.chars().count();
    (REPOSITORY_NAME_MIN_LENGTH..=REPOSITORY_NAME_MAX_LENGTH).contains(&length)
        // `\w` in a Smithy (ECMA-262) pattern is ASCII `[A-Za-z0-9_]`, so a
        // non-ASCII letter or digit does NOT satisfy `RepositoryName`.
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
        // The one name rule the model cannot express, which the operations that
        // take a name apply too: "The suffix .git is prohibited." A name no
        // repository can ever carry is not a valid repository name here either.
        && !has_prohibited_git_suffix(candidate)
}

/// Why a `resourceArn` cannot address a CodeCommit resource. Each variant is the
/// modelled error the tagging operations answer for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnProblem {
    /// The ARN is absent or blank: `ResourceArnRequiredException`.
    Missing,
    /// The value is not an ARN of a CodeCommit resource at all:
    /// `InvalidResourceArnException`.
    Invalid,
    /// A well-formed CodeCommit ARN whose resource portion is not a valid
    /// repository name: `InvalidRepositoryNameException`.
    RepositoryName,
}

/// The number of digits in an AWS account id, the fourth field of an ARN.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The repository a CodeCommit resource ARN names.
///
/// The only resource CodeCommit's tagging operations address is a repository, and
/// its ARN is `arn:<partition>:codecommit:<region>:<account-id>:<repository-name>`
/// — the form [`Repository::new`] stores and every CodeCommit response reports.
/// Anything else is refused; see [`ResourceArnProblem`] for which modelled error
/// each kind of refusal carries. Region and account are checked for FORM only:
/// this emulator's store holds one set of repositories rather than one per region
/// or account, so a well-formed ARN is resolved by the repository it names.
pub fn repository_name_of_resource_arn(raw: &str) -> Result<&str, ResourceArnProblem> {
    if raw.trim().is_empty() {
        return Err(ResourceArnProblem::Missing);
    }
    // An ARN is six colon-separated fields, `arn:<partition>:<vendor>:<region>:
    // <account>:<resource>`; `splitn` leaves the whole resource — any further
    // colons included — as the last of them, so a multi-segment resource is read
    // as one value and then refused by the repository-name rule below.
    let [prefix, partition, vendor, region, account, resource] =
        raw.splitn(6, ':').collect::<Vec<_>>()[..]
    else {
        return Err(ResourceArnProblem::Invalid);
    };
    if prefix != "arn"
        || partition.is_empty()
        // The ARN namespace is a fixed lowercase token, so a value differing in
        // case names another vendor.
        || vendor != "codecommit"
        || region.is_empty()
        || account.len() != ACCOUNT_ID_DIGITS
        || !account.bytes().all(|byte| byte.is_ascii_digit())
        || resource.is_empty()
    {
        return Err(ResourceArnProblem::Invalid);
    }
    if !is_repository_name(resource) {
        return Err(ResourceArnProblem::RepositoryName);
    }
    Ok(resource)
}

/// The message AWS returns with `RepositoryDoesNotExistException`: the requested
/// repository name, then `" for "`, then the account the lookup ran in.
///
/// CodeCommit words each of its request-derived client errors with one fixed
/// template per error code, so this crate builds each such message in exactly one
/// place and every operation raising that code goes through it — two operations
/// cannot word the same code differently, and a client that matches on the message
/// reads the same text it reads against AWS.
pub fn repository_does_not_exist_message(name: &str) -> String {
    format!("{name} for {}", ::engine::emulator_account_id())
}

/// `ApprovalRuleTemplateDoesNotExistException`'s message, built from the requested
/// template name — the one place this crate words that code, for the same reason
/// [`repository_does_not_exist_message`] is the one place it words its own.
///
/// CodeCommit words this code with one template whose single slot is the name the
/// request asked for, and whose second sentence names the likeliest cause (a call
/// made in another Region than the one the template was created in). The template
/// is the requirement; the name in the slot comes from the request.
pub fn approval_rule_template_does_not_exist_message(name: &str) -> String {
    format!(
        "No approval rule template exists in this AWS Region with the following name: {name}. \
         Make sure that you are signed in to the AWS Region where you created the rule, and \
         then try again."
    )
}

/// `ApprovalRuleTemplateNameAlreadyExistsException`'s message: a FIXED sentence
/// carrying no value from the request.
///
/// CodeCommit scopes approval-rule-template names per ACCOUNT and words this code
/// with one constant that names neither the template nor a Region, so the operations
/// that refuse a name the account already holds — `CreateApprovalRuleTemplate` and
/// `UpdateApprovalRuleTemplateName` — cannot word it differently or interpolate the
/// requested name into it.
pub const APPROVAL_RULE_TEMPLATE_NAME_ALREADY_EXISTS_MESSAGE: &str =
    "The approval rule template cannot be created because a template with that name already \
     exists in your AWS account.";

/// `ApprovalRuleTemplateInUseException`'s message: a FIXED sentence group carrying
/// no value from the request.
///
/// CodeCommit documents the cause on the error shape itself — "The approval rule
/// template is associated with one or more repositories. You cannot delete a
/// template that is associated with a repository. Remove all associations, and then
/// try again." — and words it with one constant that names neither the template nor
/// any of the repositories holding it, so a caller is told the rule and the way out
/// rather than which association it tripped over.
pub const APPROVAL_RULE_TEMPLATE_IN_USE_MESSAGE: &str =
    "The approval rule template cannot be deleted because it is associated with one or more \
     repositories. You cannot delete a template if it is associated with a repository. \
     Disassociate the template from all repositories in your AWS account, and then try again.";

/// Is this an approval rule template NAME one that can name a template at all?
///
/// `ApprovalRuleTemplateName`'s `@length` 1..100 is refused at the request boundary
/// (`crate::service::refusal_for`); this is the part of the rule the model cannot
/// express, documented on `InvalidApprovalRuleTemplateNameException`: "Template
/// names must be between 1 and 100 valid characters in length." A name of nothing
/// but whitespace names no template, and neither does one carrying a control
/// character.
///
/// The ONE place this crate decides it, so every operation that takes a template
/// name — minting one, updating one, deleting one — agrees about which names are
/// valid, and none of them can accept a name another would refuse.
pub fn is_approval_rule_template_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.chars().any(char::is_control)
}

/// `InvalidApprovalRuleTemplateNameException`'s message, built from the name the
/// request carried — the one place this crate words that code for a name
/// [`is_approval_rule_template_name`] refuses.
pub fn invalid_approval_rule_template_name_message(name: &str) -> String {
    format!("the approval rule template name {name} is not valid")
}

/// `InvalidApprovalRuleTemplateContentException`'s (and
/// `InvalidApprovalRuleContentException`'s) message: a FIXED sentence pair carrying
/// nothing from the request and no diagnostic from whichever JSON reader rejected
/// the document.
///
/// A parser's own complaint ("key must be a string at line 1 column 3") is an
/// implementation detail of the deserializer this emulator links, so it is never
/// what the caller is told: [`is_approval_rule_template_content`] answers whether
/// the document is one, and this constant is what the refusal says.
pub const INVALID_APPROVAL_RULE_CONTENT_MESSAGE: &str =
    "The JSON in the approval rule template is not valid. Review the JSON and make sure it is \
     valid, and then try again.";

/// `InvalidResourceArnException`'s message: a FIXED sentence that echoes no ARN.
///
/// The tagging operations (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) all refuse a `resourceArn` that is not an ARN of a
/// CodeCommit resource with this one constant; the malformed value the caller sent
/// is NOT interpolated back into it.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Inputted resource arn is not formatted correctly.";

/// `CommentContentRequiredException`'s message: a FIXED sentence pair that states
/// the failure and then the rule.
///
/// Every operation that saves comment content (`PostCommentForComparedCommit`,
/// `PostCommentForPullRequest`, `PostCommentReply`, `UpdateComment`) refuses empty
/// content with this one constant.
pub const COMMENT_CONTENT_REQUIRED_MESSAGE: &str =
    "The comment could not be saved because no content was provided. Comments cannot be empty.";

/// The message `InvalidBranchNameException` carries for a `branchName` that is
/// present but EMPTY — a FIXED sentence, distinct from the wording of a branch name
/// that is non-empty and still not a valid reference name.
///
/// `BranchName` is `@length min 1`, so an empty value is refused at the request
/// boundary; this is what that refusal says (see `crate::service`), rather than a
/// generic constraint-violation text.
pub const EMPTY_BRANCH_NAME_MESSAGE: &str = "Branch name cannot be empty";

/// `RepositoryNameExistsException`'s message: `Repository named <name> already
/// exists`, built from the requested name.
pub fn repository_name_exists_message(repository_name: &str) -> String {
    format!("Repository named {repository_name} already exists")
}

/// `BranchDoesNotExistException`'s message. The template has no slot: CodeCommit
/// returns this constant and names neither the branch nor the repository, because
/// a reference that resolves to a tag rather than a branch answers the very same
/// message — an ambiguity clients are written against.
pub const BRANCH_DOES_NOT_EXIST_MESSAGE: &str = "The specified branch does not exist";

/// The combined size of all the file content one commit may carry, in bytes.
/// CodeCommit documents this on `FileContentSizeLimitExceededException`: "The
/// maximum file size is 6 MB, and the combined file content change size is 7 MB."
/// (The per-file 6 MB is `FileContent`'s modelled maximum, refused at the
/// boundary.)
pub const MAX_COMBINED_CONTENT_BYTES: usize = 7 * 1024 * 1024;

/// How many approval rule templates the store holds before
/// `CreateApprovalRuleTemplate` refuses with the modelled
/// `NumberOfRuleTemplatesExceededException`.
///
/// The emulator's OWN bound on the template map rather than a claim about any
/// account's adjustable quota: a caller creates one template per request, so this
/// is what stops the map growing without end. Every record it holds is released by
/// `DeleteApprovalRuleTemplate`.
pub const APPROVAL_RULE_TEMPLATE_QUOTA: usize = 1_000;

/// How many approval rule templates one repository may be associated with before
/// `AssociateApprovalRuleTemplateWithRepository` refuses with the modelled
/// `MaximumRuleTemplatesAssociatedWithRepositoryException`.
///
/// The emulator's OWN bound on one repository's association set, not a published
/// AWS quota; `DisassociateApprovalRuleTemplateFromRepository` is its release path.
pub const MAX_TEMPLATES_PER_REPOSITORY: usize = 100;

/// How many file entries one `CreateCommit` may carry before it refuses with the
/// modelled `MaximumFileEntriesExceededException`.
///
/// The emulator's OWN bound, not a published AWS quota: the entry lists are
/// caller-sized and each one is hashed into the commit's tree, so this is what
/// bounds the per-request work.
pub const MAX_FILE_ENTRIES_PER_COMMIT: usize = 100;

/// The smallest `maxResults` a paginated CodeCommit list operation accepts; one
/// below it is refused with `InvalidMaxResultsException`. CodeCommit documents the
/// member as "a non-zero, non-negative integer" and words the refusal "A valid
/// limit is between 1 and 1000".
pub const MIN_MAX_RESULTS: i32 = 1;

/// The largest `maxResults` a paginated CodeCommit list operation accepts — the
/// upper end of the same published range (see [`MIN_MAX_RESULTS`]).
pub const MAX_MAX_RESULTS: i32 = 1_000;

/// The page size a paginated list operation uses when the caller names no
/// `maxResults`: the largest page the service allows, so an unpaginated caller sees
/// everything in one answer.
pub const DEFAULT_MAX_RESULTS: i32 = MAX_MAX_RESULTS;

/// `InvalidMaxResultsException`'s message: the range of `maxResults` the operation
/// accepts, and nothing from the request.
///
/// One of the two places this crate words that code (the other is
/// [`NON_POSITIVE_MAX_RESULTS_MESSAGE`], for an operation that publishes no
/// maximum). The template is `A valid limit is between <min> and <max>` — the
/// sentence CodeCommit answers, which states the accepted range and does NOT echo
/// the rejected value. The bounds are arguments because an operation publishes its
/// own `maxResults` maximum (the paginated list operations' is
/// [`MAX_MAX_RESULTS`], and the recorded instance of this message is that
/// 1..[`MAX_MAX_RESULTS`] one; `GetCommentsForComparedCommit`'s is
/// [`COMMENTS_PAGE_MAX`]), so the TEMPLATE is what every such operation shares.
pub fn invalid_max_results_message(min: i32, max: i32) -> String {
    format!("A valid limit is between {min} and {max}")
}

/// `InvalidMaxResultsException`'s message where the operation publishes NO maximum
/// and so refuses only a value that is not a page size at all.
///
/// `GetDifferences` and `ListFileCommitHistory` document `maxResults`/`MaxResults`
/// as "A non-zero, non-negative integer used to limit the number of returned
/// results" and serve every positive value (shrunk to the page this service
/// builds), so there is no upper bound to state and this says the rule the member's
/// own documentation states instead. NOT a recorded message: the recording for this
/// code is the range form in [`invalid_max_results_message`], which an operation
/// with a published maximum answers. What is required of this refusal is its code
/// and its 400 status.
pub const NON_POSITIVE_MAX_RESULTS_MESSAGE: &str =
    "A valid limit is a non-zero, non-negative integer";

/// `InvalidContinuationTokenException`'s message: a template whose one slot is the
/// token the request sent, led by a capital `E`.
///
/// The one place this crate words that code, so the paginated listings that page
/// through [`page_of`] and the operations that carry their own cursor cannot word it
/// differently.
pub fn invalid_continuation_token_message(token: &str) -> String {
    format!("Error parsing next token: {token}")
}

/// `CommitIdRequiredException`'s message: the sentence the model documents for the
/// shape, naming nothing from the request. The one place this crate words that code,
/// for the request boundary (`crate::service`) and for the handlers that check a
/// commit id the model leaves unconstrained alike.
pub const COMMIT_ID_REQUIRED_MESSAGE: &str = "A commit ID was not specified.";

/// `InvalidCommitIdException`'s message: likewise the shape's own documented
/// sentence, worded here once for every operation that raises the code.
pub const INVALID_COMMIT_ID_MESSAGE: &str = "The specified commit ID is not valid.";

/// The file a commit leaves behind in a folder that its deletions emptied, when the
/// caller asked to keep the folder. CodeCommit documents `keepEmptyFolders`: "If
/// true, a ..gitkeep file is created for empty folders."
pub const KEEP_EMPTY_FOLDERS_PLACEHOLDER: &str = ".gitkeep";

/// How many distinct principals' reactions one comment holds before
/// `PutCommentReaction` refuses a new one.
///
/// CodeCommit documents the per-user half of this limit — "Reactions are limited
/// to one reaction per user for each individual comment ID" — and this is the
/// emulator's OWN bound on the number of such users, because the principal a
/// reaction is keyed by comes from the request's own credential and would
/// otherwise let one caller grow a comment's reaction map without end. A caller
/// already holding a reaction always gets to change it; it is only a new
/// principal past the cap that is refused, with the modelled
/// `ReactionLimitExceededException`.
pub const MAX_REACTIONS_PER_COMMENT: usize = 100;

/// The emoji reactions CodeCommit accepts on a comment.
///
/// One row per reaction: the form this emulator reports it under (the markdown
/// shortcode), then every form a caller may send it as — the shortcode aliases
/// and the emoji character itself. The catalog is the emoji reaction table in the
/// CodeCommit User Guide, which the model's `reactionValue` documentation links to
/// (`smithy_query(query="doc",shape="PutCommentReactionInput",member="reactionValue")`).
pub const REACTION_TABLE: &[(&str, &[&str])] = &[
    (":thumbsup:", &[":thumbsup:", ":+1:", "\u{1f44d}"]),
    (":thumbsdown:", &[":thumbsdown:", ":-1:", "\u{1f44e}"]),
    (":smile:", &[":smile:", "\u{1f604}"]),
    (":confused:", &[":confused:", "\u{1f615}"]),
    (":eyes:", &[":eyes:", "\u{1f440}"]),
    (":heart:", &[":heart:", "\u{2764}"]),
    (":hooray:", &[":hooray:", ":tada:", "\u{1f389}"]),
    (":rocket:", &[":rocket:", "\u{1f680}"]),
];

/// The reaction value that REMOVES the caller's reaction rather than naming one:
/// "To remove a reaction, provide a value of blank or null. You can also provide
/// the value of none."
/// (`smithy_query(query="doc",shape="PutCommentReactionInput",member="reactionValue")`).
pub const REACTION_NONE: &str = "none";

/// The form this service reports a reaction value under, or `None` when the
/// reaction table holds no such reaction.
///
/// A caller may send a reaction as any of the forms the table lists, so the value
/// is first reduced to one comparable form: trimmed, lowercased, and with the
/// Unicode variation selector an emoji may carry (U+FE0F) removed, so `"❤️"` and
/// `"❤"` are the one reaction the table lists.
pub fn canonical_reaction(value: &str) -> Option<&'static str> {
    let normalized: String = value
        .trim()
        .chars()
        .filter(|character| *character != '\u{fe0f}')
        .flat_map(char::to_lowercase)
        .collect();
    REACTION_TABLE.iter().find_map(|(canonical, forms)| {
        forms
            .iter()
            .any(|form| *form == normalized)
            .then_some(*canonical)
    })
}

/// Does this reaction value ask for the caller's reaction to be REMOVED rather
/// than naming one? Blank, or the documented value `none`.
pub fn is_reaction_removal(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case(REACTION_NONE)
}

/// Per-service state for com-amazonaws-codecommit-codecommit-20150413. A handler reaches it through [`service_state`].
///
/// Bounded on every axis a caller drives: at most [`REPOSITORY_QUOTA`]
/// repositories, each holding at most [`MAX_FILES_PER_TREE`] files per tree,
/// [`MAX_COMMITS_RETAINED`] projected commits, [`MAX_OBJECTS_PER_REPOSITORY`]
/// git objects, [`MAX_BRANCHES_PER_REPOSITORY`] branches,
/// [`MAX_TAGS_PER_REPOSITORY`] tags and [`MAX_COMMENTS_PER_REPOSITORY`] comments,
/// with every object no ref reaches released by [`Repository::prune_history`], and
/// at most [`APPROVAL_RULE_TEMPLATE_QUOTA`] approval rule templates.
#[derive(Debug, Default)]
pub struct ComAmazonawsCodecommitCodecommit20150413State {
    /// Live repositories, keyed by `repositoryName` (unique per account).
    pub repositories: BTreeMap<String, Repository>,
    /// Live approval rule templates, keyed by `approvalRuleTemplateName` (unique
    /// per account per Region). Which repositories a template is associated with
    /// is held once, on the repository ([`Repository::associated_templates`]), so
    /// the two sides of an association cannot disagree.
    pub approval_rule_templates: BTreeMap<String, ApprovalRuleTemplateRecord>,
}

impl ComAmazonawsCodecommitCodecommit20150413State {
    /// The comment this `commentId` names, wherever it was posted.
    ///
    /// A comment id is addressable on its own — `GetComment` takes nothing else —
    /// while a comment's lifetime is its repository's, so the comments live on the
    /// repository that holds them and are found through this one lookup rather
    /// than through a second index that could disagree with it.
    pub fn comment(&self, comment_id: &str) -> Option<&CommentRecord> {
        self.repositories
            .values()
            .find_map(|repository| repository.comments.get(comment_id))
    }

    /// The same lookup, for a caller that goes on to modify the comment.
    pub fn comment_mut(&mut self, comment_id: &str) -> Option<&mut CommentRecord> {
        self.repositories
            .values_mut()
            .find_map(|repository| repository.comments.get_mut(comment_id))
    }

    /// Is this approval rule template associated with any repository?
    ///
    /// An association is held once, on the repository
    /// ([`Repository::associated_templates`]), so the answer is a scan of the
    /// repository map — the same stored association
    /// `ListRepositoriesForApprovalRuleTemplate` reports and
    /// `DeleteApprovalRuleTemplate` refuses on, rather than a second index that
    /// could disagree with it.
    pub fn template_is_associated(&self, template_name: &str) -> bool {
        self.repositories
            .values()
            .any(|repository| repository.associated_templates.contains(template_name))
    }
}

/// One repository and everything reachable from it.
#[derive(Debug, Clone)]
pub struct Repository {
    pub repository_id: String,
    pub account_id: String,
    pub name: String,
    /// The region this repository lives in, as the request that created it was
    /// scoped for. Held because the ARN and both clone URLs are built from the
    /// region AND the name, so a rename has to rebuild them for the SAME region.
    pub region: String,
    pub description: Option<String>,
    pub default_branch: Option<String>,
    pub arn: String,
    pub kms_key_id: Option<String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// Branches (the `refs/heads/` namespace), keyed by branch name.
    pub branches: BTreeMap<String, Branch>,
    /// Tags (the `refs/tags/` namespace), keyed by tag name, each naming the
    /// object the tag points at. CodeCommit has no control-plane API that
    /// creates a tag; a tag gets here through the git transport.
    pub git_tags: BTreeMap<String, String>,
    /// The commit PROJECTION: one record per commit the control plane can read.
    pub commits: BTreeMap<String, Commit>,
    /// The git object store: every blob, tree, commit and tag this repository
    /// holds, keyed by object name.
    pub objects: BTreeMap<String, Object>,
    /// The names of the approval rule templates associated with this repository.
    /// A set, so associating the same template twice is the no-op CodeCommit makes
    /// it, and bounded by [`MAX_TEMPLATES_PER_REPOSITORY`].
    pub associated_templates: BTreeSet<String>,
    /// The sequence number the next commit takes, so the oldest commits can be
    /// told from the newest when [`Repository::prune_history`] runs.
    pub next_commit_sequence: u64,
    /// Comments posted on this repository, keyed by `commentId`.
    pub comments: BTreeMap<String, CommentRecord>,
    /// Each `clientRequestToken` a comment was posted with, mapped to the
    /// `commentId` that first request created, so a repeated request carrying the
    /// same token returns that comment instead of posting a second one.
    pub comment_tokens: BTreeMap<String, String>,
    /// The sequence number the next comment takes: comments are reported in the
    /// order they were posted.
    pub next_comment_sequence: u64,
}

/// A branch: its name and the commit it points at (its tip).
#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub commit_id: String,
}

/// A commit. `files` is the FULL tree snapshot at this commit, so reading a file
/// at any commit specifier is a lookup rather than a replay of history.
#[derive(Debug, Clone)]
pub struct Commit {
    pub commit_id: String,
    pub tree_id: String,
    pub parents: Vec<String>,
    pub message: Option<String>,
    pub author: UserInfo,
    pub committer: UserInfo,
    pub files: BTreeMap<String, FileEntry>,
    /// Write order within the repository: the lowest sequence is the oldest
    /// commit, which is what [`Repository::prune_history`] drops first.
    pub sequence: u64,
}

/// One file in a commit's tree.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub blob_id: String,
    pub file_mode: String,
    pub size: i64,
}

/// The host a URL this emulator announces names when the asking request carries
/// no address at all — only an in-process caller that builds a path-only URI with
/// no `Host` does. Loopback is where the emulator binds by default
/// (`EMULATOR_BIND_ADDRESS`), so the announced URL still names this emulator
/// rather than a host it does not serve; it is the fallback
/// `service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader` uses for the
/// same question about its own playback URL.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The endpoint a client reached this emulator at: the scheme and authority
/// every URL the emulator announces for itself has to name.
///
/// CodeCommit's `cloneUrlHttp` is the address the repository's git transport
/// answers on, so in an emulator it is the address THIS emulator answers on —
/// the endpoint the request arrived at — and not the public AWS git host, which
/// is a different server entirely and would send a client that follows the URL
/// off the emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingEndpoint {
    pub scheme: String,
    pub authority: String,
}

impl ServingEndpoint {
    /// The endpoint this request arrived at: the `Host` header (or, on HTTP/2,
    /// the URI authority), under the scheme the request names.
    ///
    /// A request that carries neither — only an in-process caller that builds a
    /// path-only URI with no `Host` — falls back to [`LOOPBACK_HOST`], so every
    /// URL this service announces names an address this emulator serves and never
    /// one it does not.
    pub fn from_request(parts: &::http::request::Parts) -> Self {
        let authority = parts
            .headers
            .get(::http::header::HOST)
            .and_then(|host| host.to_str().ok())
            .map(str::to_string)
            .or_else(|| parts.uri.authority().map(|a| a.as_str().to_string()))
            .filter(|authority| !authority.is_empty())
            .unwrap_or_else(|| LOOPBACK_HOST.to_string());
        let scheme = parts
            .uri
            .scheme_str()
            .or_else(|| {
                parts
                    .headers
                    .get("x-forwarded-proto")
                    .and_then(|proto| proto.to_str().ok())
            })
            // The emulator's own listener serves HTTPS, which is also the only
            // scheme CodeCommit publishes a clone URL for.
            .unwrap_or("https")
            .to_string();
        Self { scheme, authority }
    }

    /// The HTTPS clone URL for a repository: the git smart-HTTP endpoint this
    /// emulator serves for it (see [`crate::service::git_http`]).
    pub fn clone_url_http(&self, name: &str) -> String {
        format!("{}://{}/v1/repos/{name}", self.scheme, self.authority)
    }

    /// The SSH clone URL for a repository, naming the same host.
    ///
    /// The emulator serves no SSH transport — there is no SSH listener to bind —
    /// so this URL names a host that answers git over HTTPS only. It still names
    /// the emulator rather than the public AWS host, because a client that
    /// follows it must fail against the emulator instead of being sent to a real
    /// AWS endpoint where its credentials do not exist. The port is omitted: a
    /// port in the authority is the HTTP listener's, which an SSH URL must not
    /// claim.
    pub fn clone_url_ssh(&self, name: &str) -> String {
        format!("ssh://{}/v1/repos/{name}", self.host())
    }

    /// The authority with any `:port` removed.
    fn host(&self) -> &str {
        let after_userinfo = self
            .authority
            .rsplit_once('@')
            .map_or(self.authority.as_str(), |(_, host)| host);
        if after_userinfo.starts_with('[') {
            // An IPv6 literal keeps its brackets; only a port after them is cut.
            return after_userinfo
                .rsplit_once("]:")
                .map_or(after_userinfo, |(host, _)| &after_userinfo[..host.len() + 1]);
        }
        after_userinfo
            .rsplit_once(':')
            .map_or(after_userinfo, |(host, _)| host)
    }
}

impl Commit {
    /// This commit as the modeled `Commit` shape — the one place that projection
    /// is written, so every operation that reports a stored commit agrees about
    /// it, exactly as [`Repository::metadata`] does for a repository.
    ///
    /// `parents` is always present, empty for a root commit, because the model's
    /// `ParentList` is a list of the full ids of the commits this one was built
    /// on. `additionalData` carries whatever a git host records beyond the
    /// modelled members; this emulator's commit objects record none, so it is
    /// absent rather than empty.
    pub fn info(&self) -> crate::ops::types::Commit {
        crate::ops::types::Commit {
            commit_id: Some(self.commit_id.clone()),
            tree_id: Some(self.tree_id.clone()),
            parents: Some(self.parents.clone()),
            message: self.message.clone(),
            author: Some(self.author.clone()),
            committer: Some(self.committer.clone()),
            additional_data: None,
        }
    }

    /// The id of the blob this commit's tree holds at `path`, or `None` where the
    /// tree does not hold that path at all.
    pub fn blob_at(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|entry| entry.blob_id.as_str())
    }
}

/// The comparison a comment was posted on: the two commits that establish its
/// directionality, and the blobs of the commented file in each of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparedCommit {
    pub before_commit_id: Option<String>,
    pub after_commit_id: String,
    pub before_blob_id: Option<String>,
    pub after_blob_id: Option<String>,
}

/// One comment, as posted. `comparison` is what the comment is filed under, so a
/// read of one comparison reports exactly the comments posted on it.
#[derive(Debug, Clone)]
pub struct CommentRecord {
    pub comment_id: String,
    pub content: String,
    /// The content the comment was POSTED with, which an idempotency token is
    /// judged against: a token replay repeats the request the token was first
    /// accepted for, so a later edit of the comment must not turn that replay
    /// into a parameter mismatch.
    pub posted_content: String,
    /// The comment this one replies to; `None` for the root of a thread.
    pub in_reply_to: Option<String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub author_arn: String,
    pub deleted: bool,
    pub client_request_token: Option<String>,
    pub comparison: ComparedCommit,
    pub location: Option<Location>,
    /// The emoji reactions left on this comment, keyed by the ARN of the
    /// principal that left each one, so a principal holds at most one reaction
    /// per comment ("Reactions are limited to one reaction per user for each
    /// individual comment ID") and `callerReactions` is that principal's own
    /// entry. Bounded by [`MAX_REACTIONS_PER_COMMENT`] principals.
    pub reactions: BTreeMap<String, String>,
    /// Post order within the repository: the lowest sequence is the oldest
    /// comment, and that is the order comments are reported in.
    pub sequence: u64,
}

impl CommentRecord {
    /// This comment as the modeled `Comment` shape — the one place that
    /// projection is written, so `PostCommentForComparedCommit` and
    /// `GetCommentsForComparedCommit` cannot disagree about a stored comment.
    pub fn comment(&self) -> Comment {
        self.comment_for(None)
    }

    /// The same projection for a NAMED caller: `callerReactions` is that
    /// principal's own reaction, which is the one member of a comment that
    /// depends on who is asking.
    pub fn comment_as(&self, caller_arn: &str) -> Comment {
        self.comment_for(Some(caller_arn))
    }

    fn comment_for(&self, caller_arn: Option<&str>) -> Comment {
        // `reactionCounts` is how many principals hold each reaction, counted
        // from the stored map rather than tallied anywhere else.
        let mut reaction_counts: HashMap<String, i32> = HashMap::new();
        for reaction in self.reactions.values() {
            *reaction_counts.entry(reaction.clone()).or_insert(0) += 1;
        }
        Comment {
            comment_id: Some(self.comment_id.clone()),
            // A deleted comment keeps its record but reports no content, which is
            // what `DeleteCommentContent` documents.
            content: (!self.deleted).then(|| self.content.clone()),
            in_reply_to: self.in_reply_to.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            author_arn: Some(self.author_arn.clone()),
            deleted: self.deleted,
            client_request_token: self.client_request_token.clone(),
            caller_reactions: caller_arn
                .and_then(|caller_arn| self.reactions.get(caller_arn))
                .map(|reaction| vec![reaction.clone()]),
            reaction_counts: (!reaction_counts.is_empty()).then_some(reaction_counts),
        }
    }
}

/// A location's members as a comparable key: two comments sit in the same place in
/// a comparison only when all three members agree. The modeled `Location` shape
/// derives no `PartialEq`, so this is how two of them are compared.
pub fn location_key(location: Option<&Location>) -> (Option<&str>, Option<i64>, Option<&str>) {
    match location {
        None => (None, None, None),
        Some(location) => (
            location.file_path.as_deref(),
            location.file_position,
            location.relative_file_version.as_deref(),
        ),
    }
}

/// How many lines a blob's bytes hold: the number of line separators, plus the
/// trailing line when the content does not end in one. This is what bounds the
/// `filePosition` a comment can name in that version of a file.
pub fn line_count(content: &[u8]) -> i64 {
    if content.is_empty() {
        return 0;
    }
    let separators = content.iter().filter(|byte| **byte == b'\n').count();
    let trailing = usize::from(!content.ends_with(b"\n"));
    i64::try_from(separators + trailing).unwrap_or(i64::MAX)
}

impl Repository {
    /// Build a repository, deriving every generated field from the name, the
    /// region and the creation instant.
    pub fn new(
        name: &str,
        description: Option<String>,
        kms_key_id: Option<String>,
        tags: BTreeMap<String, String>,
        region: &str,
        created: SystemTime,
    ) -> Self {
        Self {
            repository_id: repository_id(name, created),
            account_id: ::engine::emulator_account_id().to_string(),
            name: name.to_string(),
            region: region.to_string(),
            description,
            default_branch: None,
            arn: ::engine::emulator_arn("codecommit", region, name),
            kms_key_id,
            creation_date: created,
            last_modified_date: created,
            tags,
            branches: BTreeMap::new(),
            git_tags: BTreeMap::new(),
            commits: BTreeMap::new(),
            objects: BTreeMap::new(),
            associated_templates: BTreeSet::new(),
            next_commit_sequence: 0,
            comments: BTreeMap::new(),
            comment_tokens: BTreeMap::new(),
            next_comment_sequence: 0,
        }
    }

    /// This repository as the modeled `RepositoryMetadata` shape — the one place
    /// that projection is written, so `CreateRepository` and `GetRepository`
    /// cannot disagree about a stored repository.
    ///
    /// The clone URLs are derived from `endpoint`, the address the ASKING request
    /// arrived at, because that is the address this emulator's git transport
    /// answers on; they are not stored, so they cannot go stale when the emulator
    /// is reached on a different endpoint than the one that created the
    /// repository.
    pub fn metadata(&self, endpoint: &ServingEndpoint) -> RepositoryMetadata {
        RepositoryMetadata {
            account_id: Some(self.account_id.clone()),
            repository_id: Some(self.repository_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_description: self.description.clone(),
            default_branch: self.default_branch.clone(),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            clone_url_http: Some(endpoint.clone_url_http(&self.name)),
            clone_url_ssh: Some(endpoint.clone_url_ssh(&self.name)),
            arn: Some(self.arn.clone()),
            kms_key_id: self.kms_key_id.clone(),
        }
    }

    /// Rename this repository, recomputing every field CodeCommit derives from the
    /// repository name — the ARN addresses a repository BY name — and moving the
    /// last-modified instant to the rename. Both clone URLs name the repository too,
    /// and follow the rename for free: they are derived from the stored name and the
    /// asking request's endpoint on every read (see [`Repository::metadata`]).
    ///
    /// Everything that belongs to the resource rather than to its name (the
    /// repository id, the creation date, the description, the KMS key, the tags, the
    /// branches, the tags, the commits and the git objects) carries over untouched,
    /// so a renamed repository is the same repository under a new name. The caller
    /// re-keys the store, which is keyed by `repositoryName`.
    pub fn rename(&mut self, new_name: &str, renamed: SystemTime) {
        self.name = new_name.to_string();
        self.arn = ::engine::emulator_arn("codecommit", &self.region, new_name);
        self.last_modified_date = renamed;
    }

    /// Point this repository at another KMS encryption key, answering the key it
    /// was encrypted under before — the `originalKmsKeyId` a rotation reports,
    /// absent for a repository that carried no key of its own.
    ///
    /// Only which key the repository DEPENDS on changes: its identity, its
    /// metadata and its whole history carry over, and the last-modified instant
    /// records the change. A request naming the key already in effect changes
    /// nothing at all, so it leaves that instant where it is too.
    pub fn rotate_encryption_key(
        &mut self,
        kms_key_id: &str,
        rotated: SystemTime,
    ) -> Option<String> {
        let original = self.kms_key_id.replace(kms_key_id.to_string());
        if original.as_deref() != Some(kms_key_id) {
            self.last_modified_date = rotated;
        }
        original
    }

    /// The commit a `commitSpecifier` names. `CommitName` documents the forms as
    /// "the branch, tag, HEAD, or other fully qualified reference used to identify
    /// a commit", so a branch name, the fully qualified reference of a branch
    /// (`refs/heads/<branch>`), `HEAD` (the repository's default branch) and the id
    /// of a commit this repository holds all resolve here. `None` when the
    /// specifier names none of them. This repository holds no tags, so a tag
    /// reference resolves to nothing until tags are stored.
    pub fn resolve_commit(&self, specifier: &str) -> Option<&Commit> {
        // A branch of that literal name comes first, so the reference forms below
        // only resolve what the bare name could not — a repository that really
        // holds a branch called `HEAD` is still addressable by name.
        if let Some(branch) = self.branches.get(specifier) {
            return self.commits.get(&branch.commit_id);
        }
        let referenced = match specifier {
            "HEAD" => self.default_branch.as_deref(),
            other => other.strip_prefix("refs/heads/"),
        };
        if let Some(branch) = referenced.and_then(|name| self.branches.get(name)) {
            return self.commits.get(&branch.commit_id);
        }
        self.commits.get(specifier)
    }

    /// The object this name addresses.
    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.get(id)
    }

    /// The bytes of a blob, or `None` when this name is not a blob this
    /// repository holds.
    pub fn blob(&self, id: &str) -> Option<&[u8]> {
        self.objects
            .get(id)
            .filter(|object| object.kind == Kind::Blob)
            .map(|object| object.data.as_slice())
    }

    /// Store an object, reporting its name. Storing the same object twice is the
    /// same as storing it once: the name IS the content.
    pub fn insert_object(&mut self, object: Object) -> String {
        let id = object.id();
        self.objects.entry(id.clone()).or_insert(object);
        id
    }

    /// Store a file's bytes as a blob, reporting its `blobId`.
    pub fn insert_blob(&mut self, content: Vec<u8>) -> String {
        self.insert_object(Object::new(Kind::Blob, content))
    }

    /// Total payload bytes the object store holds.
    pub fn object_bytes(&self) -> usize {
        self.objects
            .values()
            .map(|object| object.data.len())
            .fold(0usize, usize::saturating_add)
    }

    /// Commit a whole tree snapshot: write the tree objects and the commit
    /// object, record the commit projection, and report `(commitId, treeId)`.
    ///
    /// The blobs the snapshot names must already be stored (see
    /// [`Repository::insert_blob`]), which is what makes the commit's tree
    /// complete in the object store and therefore serveable to a git client —
    /// and what makes every entry's `blobId` an object name, so the tree encodes.
    /// `None` when one does not.
    pub fn commit_snapshot(
        &mut self,
        files: BTreeMap<String, FileEntry>,
        parents: Vec<String>,
        author: UserInfo,
        committer: UserInfo,
        message: Option<&str>,
    ) -> Option<(String, String)> {
        let (tree_id, trees) = tree_objects(&files)?;
        for tree in trees {
            self.insert_object(tree);
        }
        let commit = git_object::encode_commit(&git_object::CommitFields {
            tree: tree_id.clone(),
            parents: parents.clone(),
            author: git_identity(&author),
            committer: git_identity(&committer),
            message: message.unwrap_or_default().to_string(),
        });
        let commit_id = self.insert_object(commit);
        let sequence = self.next_commit_sequence;
        self.next_commit_sequence = sequence.saturating_add(1);
        self.commits.insert(
            commit_id.clone(),
            Commit {
                commit_id: commit_id.clone(),
                tree_id: tree_id.clone(),
                parents,
                message: message.map(str::to_string),
                author,
                committer,
                files,
                sequence,
            },
        );
        Some((commit_id, tree_id))
    }

    /// Project the commit object `id` — and its ancestors, as far as the objects
    /// present and `budget` allow — into [`Repository::commits`], so the control
    /// plane can read what the git transport stored.
    ///
    /// `budget` is ONE request's whole projection allowance, not one call's, so a
    /// request carrying many ref updates cannot multiply this work: see
    /// [`ProjectionBudget`].
    ///
    /// `Err` names the first commit whose tree the projection cannot hold, or the
    /// budget it ran past, which is what lets a push be refused instead of
    /// accepted into a state the control plane could not read.
    pub fn project_history(
        &mut self,
        id: &str,
        budget: &mut ProjectionBudget,
    ) -> Result<(), String> {
        // Oldest-last walk from the tip, stopping at a commit already projected.
        let mut order: Vec<String> = Vec::new();
        let mut queue: Vec<String> = vec![id.to_string()];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(next) = queue.pop() {
            if !seen.insert(next.clone()) || self.commits.contains_key(&next) {
                continue;
            }
            if order.len() >= MAX_COMMITS_RETAINED {
                break;
            }
            let Some(object) = self.objects.get(&next) else {
                // An ancestor this repository does not hold: the history ends
                // here as far as the projection is concerned.
                continue;
            };
            if object.kind != Kind::Commit {
                return Err(format!("{next} is not a commit"));
            }
            let fields = git_object::decode_commit(&object.data)
                .ok_or_else(|| format!("{next} is not a well-formed commit object"))?;
            order.push(next);
            for parent in fields.parents {
                queue.push(parent);
            }
        }
        // Ancestors first, so a commit's sequence number grows with history and
        // `prune_history` drops the oldest first.
        for commit_id in order.into_iter().rev() {
            self.project_commit(&commit_id, budget)?;
        }
        Ok(())
    }

    /// Project one commit object into [`Repository::commits`].
    fn project_commit(&mut self, id: &str, budget: &mut ProjectionBudget) -> Result<(), String> {
        budget.commit()?;
        let object = self
            .objects
            .get(id)
            .ok_or_else(|| format!("the object {id} is missing"))?;
        let fields = git_object::decode_commit(&object.data)
            .ok_or_else(|| format!("{id} is not a well-formed commit object"))?;
        let files = self.flatten_tree(&fields.tree, budget)?;
        let (author_name, author_email, author_date) = git_object::split_identity(&fields.author);
        let (committer_name, committer_email, committer_date) =
            git_object::split_identity(&fields.committer);
        let sequence = self.next_commit_sequence;
        self.next_commit_sequence = sequence.saturating_add(1);
        self.commits.insert(
            id.to_string(),
            Commit {
                commit_id: id.to_string(),
                tree_id: fields.tree,
                parents: fields.parents,
                message: Some(fields.message).filter(|message| !message.is_empty()),
                author: UserInfo {
                    name: Some(author_name),
                    email: Some(author_email),
                    date: Some(author_date),
                },
                committer: UserInfo {
                    name: Some(committer_name),
                    email: Some(committer_email),
                    date: Some(committer_date),
                },
                files,
                sequence,
            },
        );
        Ok(())
    }

    /// The flattened snapshot a tree object describes: every file in it, keyed by
    /// its path from the root.
    ///
    /// A gitlink entry (a submodule's commit embedded in a tree) names no blob
    /// and so is not a file; it is left out of the snapshot.
    ///
    /// The walk is charged to `budget`, ONE request's whole projection allowance,
    /// entry by entry — NOT just bounded by the depth and the file count: a tree
    /// is a DAG, so a handful of pushed tree objects can name one another and
    /// present exponentially many paths while inserting no file at all (every leaf
    /// a gitlink, or an empty tree), and neither the depth nor the file count
    /// would ever trip. A visited set would be the wrong bound here: two
    /// directories with identical content legitimately SHARE one tree object, and
    /// both have to appear in the snapshot, so a repeated subtree must be walked
    /// again — what is refused is the total work, not the repetition.
    pub fn flatten_tree(
        &self,
        root: &str,
        budget: &mut ProjectionBudget,
    ) -> Result<BTreeMap<String, FileEntry>, String> {
        let mut files = BTreeMap::new();
        self.flatten_tree_into(root, "", 0, budget, &mut files)?;
        Ok(files)
    }

    fn flatten_tree_into(
        &self,
        tree_id: &str,
        prefix: &str,
        depth: usize,
        budget: &mut ProjectionBudget,
        files: &mut BTreeMap<String, FileEntry>,
    ) -> Result<(), String> {
        if depth >= MAX_PATH_SEGMENTS {
            return Err(format!(
                "the tree {tree_id} is nested deeper than {MAX_PATH_SEGMENTS} levels"
            ));
        }
        let object = self
            .objects
            .get(tree_id)
            .ok_or_else(|| format!("the tree {tree_id} is missing"))?;
        if object.kind != Kind::Tree {
            return Err(format!("{tree_id} is not a tree"));
        }
        let entries = git_object::decode_tree(&object.data)
            .ok_or_else(|| format!("{tree_id} is not a well-formed tree object"))?;
        for entry in entries {
            // Every entry costs one, whether or not it becomes a file, which is
            // what bounds a shared-subtree DAG.
            budget.entry()?;
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if entry.is_tree() {
                self.flatten_tree_into(&entry.id, &path, depth + 1, budget, files)?;
                continue;
            }
            if entry.mode == git_object::MODE_GITLINK {
                continue;
            }
            let size = self
                .objects
                .get(&entry.id)
                .filter(|object| object.kind == Kind::Blob)
                .ok_or_else(|| format!("the blob {} for {path} is missing", entry.id))?
                .data
                .len();
            if files.len() >= MAX_FILES_PER_TREE {
                return Err(format!(
                    "the tree {tree_id} holds more than the {MAX_FILES_PER_TREE} files this emulator keeps per tree"
                ));
            }
            files.insert(
                path,
                FileEntry {
                    blob_id: entry.id,
                    file_mode: file_mode_of(&entry.mode).to_string(),
                    size: i64::try_from(size).unwrap_or(i64::MAX),
                },
            );
        }
        Ok(())
    }

    /// Every commit id reachable from `commit_id` by following parents, including
    /// `commit_id` itself: the history a read at that commit sees. An id whose
    /// commit [`Repository::prune_history`] has dropped is still reported, because
    /// a retained commit's `parents` keeps naming it.
    pub fn ancestry(&self, commit_id: &str) -> BTreeSet<String> {
        let mut reachable = BTreeSet::new();
        let mut pending = vec![commit_id.to_string()];
        while let Some(id) = pending.pop() {
            if !reachable.insert(id.clone()) {
                continue;
            }
            if let Some(commit) = self.commits.get(&id) {
                pending.extend(commit.parents.iter().cloned());
            }
        }
        reachable
    }

    /// Every ref this repository holds, as `(full refname, object name)`, in
    /// refname order — the order a git client is advertised them in.
    pub fn refs(&self) -> Vec<(String, String)> {
        let mut refs: Vec<(String, String)> = self
            .branches
            .values()
            .map(|branch| (format!("{HEADS_PREFIX}{}", branch.name), branch.commit_id.clone()))
            .chain(
                self.git_tags
                    .iter()
                    .map(|(name, id)| (format!("{TAGS_PREFIX}{name}"), id.clone())),
            )
            .collect();
        refs.sort();
        refs
    }

    /// The commit an annotated tag (or a chain of them) finally points at, or
    /// `None` when the object is not a tag and not a commit this repository
    /// holds.
    pub fn peel(&self, id: &str) -> Option<String> {
        let mut current = id.to_string();
        // A tag of a tag of a tag … is bounded, so a cycle cannot spin here.
        for _ in 0..16 {
            let object = self.objects.get(&current)?;
            match object.kind {
                Kind::Commit => return Some(current),
                Kind::Tag => current = git_object::decode_tag(&object.data)?.object,
                _ => return None,
            }
        }
        None
    }

    /// The first object name `roots` reach that this repository does NOT hold, or
    /// `None` when the closure is complete.
    ///
    /// What a git host checks before it moves a ref: a ref whose closure is
    /// incomplete is one a later fetch cannot serve, so the push that would create
    /// it is refused instead.
    pub fn first_missing_object(&self, roots: &[String]) -> Option<String> {
        self.reachable_from(roots)
            .into_iter()
            .find(|id| !self.objects.contains_key(id))
    }

    /// [`Repository::first_missing_object`], charged to ONE request's budget.
    ///
    /// The form a caller-driven walk uses. A single request may ask for this once
    /// per ref it updates, so every walk charges the same [`ProjectionBudget`] —
    /// what is bounded is the whole request's walking, not each walk — and that
    /// budget's deadline stops a walk that runs long however few entries it has
    /// visited. `Err` carries the reason, which the caller reports as the refusal.
    pub fn first_missing_object_within(
        &self,
        roots: &[String],
        budget: &mut ProjectionBudget,
    ) -> Result<Option<String>, String> {
        Ok(self
            .walk_reachable(roots, Some(budget))?
            .into_iter()
            .find(|id| !self.objects.contains_key(id)))
    }

    /// Every object name reachable from `roots`, following commits to their
    /// trees and parents, trees to their entries and tags to their targets.
    ///
    /// Unbudgeted, for this service's OWN walks rather than a caller's: one call
    /// visits each object at most once, so its cost is bounded by what the
    /// repository holds ([`MAX_OBJECTS_PER_REPOSITORY`],
    /// [`MAX_OBJECT_BYTES_PER_REPOSITORY`]). A walk a REQUEST can drive repeatedly
    /// goes through [`Repository::first_missing_object_within`], which charges a
    /// budget, because the number of walks is then the caller's to choose.
    pub fn reachable_from(&self, roots: &[String]) -> BTreeSet<String> {
        // No budget is charged, so the walk cannot be refused.
        self.walk_reachable(roots, None).unwrap_or_default()
    }

    /// The one reachability walk, charged to `budget` when there is one.
    fn walk_reachable(
        &self,
        roots: &[String],
        mut budget: Option<&mut ProjectionBudget>,
    ) -> Result<BTreeSet<String>, String> {
        let mut reached: BTreeSet<String> = BTreeSet::new();
        let mut queue: Vec<String> = roots.to_vec();
        while let Some(next) = queue.pop() {
            if let Some(budget) = budget.as_deref_mut() {
                // Charged per object TAKEN from the queue, which counts the
                // entries every tree pushed onto it, so a tree naming many
                // entries costs what it enqueued.
                budget.entry()?;
            }
            if !reached.insert(next.clone()) {
                continue;
            }
            let Some(object) = self.objects.get(&next) else {
                continue;
            };
            match object.kind {
                Kind::Blob => {}
                Kind::Commit => {
                    if let Some(fields) = git_object::decode_commit(&object.data) {
                        queue.push(fields.tree);
                        queue.extend(fields.parents);
                    }
                }
                Kind::Tree => {
                    if let Some(entries) = git_object::decode_tree(&object.data) {
                        queue.extend(entries.into_iter().map(|entry| entry.id));
                    }
                }
                Kind::Tag => {
                    if let Some(fields) = git_object::decode_tag(&object.data) {
                        queue.push(fields.object);
                    }
                }
            }
        }
        Ok(reached)
    }

    /// Release what no ref reaches, and keep the commit projection bounded.
    ///
    /// Objects are kept exactly when a ref reaches them, which is what git's own
    /// garbage collection does: an object a force-push or a branch deletion
    /// orphaned is released. The projection additionally drops the oldest commits
    /// beyond [`MAX_COMMITS_RETAINED`] (never a ref's tip), so an older commit id
    /// becomes one the repository no longer holds while every ref stays readable.
    pub fn prune_history(&mut self) {
        let roots: Vec<String> = self.refs().into_iter().map(|(_, id)| id).collect();
        let reachable = self.reachable_from(&roots);
        self.objects.retain(|id, _| reachable.contains(id));
        self.commits.retain(|id, _| reachable.contains(id));

        let tips: BTreeSet<String> = roots
            .iter()
            .filter_map(|id| self.peel(id))
            .chain(roots.iter().cloned())
            .collect();
        if self.commits.len() > MAX_COMMITS_RETAINED {
            let mut oldest_first: Vec<(u64, String)> = self
                .commits
                .values()
                .map(|commit| (commit.sequence, commit.commit_id.clone()))
                .collect();
            oldest_first.sort_unstable();
            let mut excess = self.commits.len() - MAX_COMMITS_RETAINED;
            for (_, commit_id) in oldest_first {
                if excess == 0 {
                    break;
                }
                if tips.contains(&commit_id) {
                    continue;
                }
                self.commits.remove(&commit_id);
                excess -= 1;
            }
        }
    }
}

/// One approval rule template and everything CodeCommit reports about it.
///
/// Which repositories it is associated with is NOT held here: that lives once, on
/// each repository's [`Repository::associated_templates`].
#[derive(Debug, Clone)]
pub struct ApprovalRuleTemplateRecord {
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The approval rule document in its canonical form — see
    /// [`canonical_json_document`]. It is canonicalized once, here, on the way in,
    /// so every reader of a stored template reports the same bytes and
    /// `ruleContentSha256` is the digest of those bytes.
    pub content: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    /// The ARN of the identity that made the most recent change, as CodeCommit
    /// reports it in `lastModifiedUser`.
    pub last_modified_user: String,
}

impl ApprovalRuleTemplateRecord {
    /// Build a template, deriving every generated field from the name, the content
    /// and the creation instant.
    pub fn new(
        name: &str,
        content: String,
        description: Option<String>,
        last_modified_user: String,
        created: SystemTime,
    ) -> Self {
        Self {
            template_id: approval_rule_template_id(name, created),
            name: name.to_string(),
            description: normalized_template_description(description),
            // The document is held canonicalized, never as the caller happened to
            // format it: whatever the submitted whitespace, the stored content,
            // `ruleContentSha256` and every later read agree.
            content: canonical_json_document(&content),
            creation_date: created,
            last_modified_date: created,
            last_modified_user,
        }
    }

    /// Set this template's description to the one a request carried, and record
    /// who changed it when.
    ///
    /// The description is normalized by [`normalized_template_description`], so a
    /// template never holds an empty one however it was set, and the change moves
    /// `lastModifiedDate` and `lastModifiedUser` onto this call: both report the
    /// most recent change to the template, not its creation.
    pub fn set_description(
        &mut self,
        description: Option<String>,
        modified_by: String,
        modified: SystemTime,
    ) {
        self.description = normalized_template_description(description);
        self.last_modified_user = modified_by;
        self.last_modified_date = modified;
    }

    /// This template as the modeled `ApprovalRuleTemplate` shape — the one place
    /// that projection is written, so `CreateApprovalRuleTemplate`,
    /// `GetApprovalRuleTemplate` and any later updater cannot disagree about a
    /// stored template.
    pub fn to_modeled(&self) -> ApprovalRuleTemplate {
        ApprovalRuleTemplate {
            approval_rule_template_id: Some(self.template_id.clone()),
            approval_rule_template_name: Some(self.name.clone()),
            approval_rule_template_description: self.description.clone(),
            approval_rule_template_content: Some(self.content.clone()),
            rule_content_sha256: Some(content_sha256(&self.content)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_user: Some(self.last_modified_user.clone()),
        }
    }

    /// Replace this template's content, recording who changed it and when.
    ///
    /// The one place a stored template's content is overwritten, so every updater
    /// attributes the change the same way: the content is canonicalized exactly as a
    /// created template's is ([`canonical_json_document`]) — an update can therefore
    /// not leave a template in a serialization a create would never have stored —
    /// and with it the `ruleContentSha256` derived from it, while the template's id,
    /// name, description and creation date are untouched.
    pub fn replace_content(&mut self, content: String, by: String, at: SystemTime) {
        self.content = canonical_json_document(&content);
        self.last_modified_user = by;
        self.last_modified_date = at;
    }
}

/// An approval rule template's description as CodeCommit HOLDS it, whatever a
/// request carried.
///
/// A template holds no empty description: one set to an empty string simply has
/// none, so `approvalRuleTemplateDescription` is ABSENT on the answer and on every
/// later read rather than reported as an empty string (the recorded behaviour the
/// corpus names
/// "approval-rule-template-empty-description-normalized-to-absent"). Every write
/// of a description — the create and every update — goes through here, so they
/// cannot disagree about what an empty one means.
pub fn normalized_template_description(description: Option<String>) -> Option<String> {
    description.filter(|text| !text.is_empty())
}

/// The SHA-256 hash signature CodeCommit reports for a template's content
/// (`ruleContentSha256`): the digest of the content bytes, hex encoded. Those bytes
/// are the canonical ones ([`canonical_json_document`]), so the signature of a
/// document does not depend on how its submitter formatted it.
pub fn content_sha256(content: &str) -> String {
    ::hex::encode(Sha256::digest(content.as_bytes()))
}

/// A JSON document CodeCommit stores and hands back, in its canonical form:
/// minified — every byte of RFC 8259 insignificant whitespace (space, tab, CR, LF
/// between tokens) removed — with the member order, the string literals and the
/// number literals exactly as submitted. A service that accepts a JSON document and
/// reports it back does not echo the submitted formatting: a caller that stores the
/// returned text, diffs it against a later read, or hashes it to detect drift sees a
/// change only when the document itself changed.
///
/// Whitespace is dropped by scanning the text rather than by re-serializing the
/// parsed value, because a re-serialization would sort the members (`serde_json`'s
/// `Map` is a `BTreeMap` unless its `preserve_order` feature is on) and renormalise
/// number literals (`1e3` to `1000.0`), both of which this canonical form preserves.
/// Whitespace inside a string literal is part of the value, so it survives, escapes
/// included.
///
/// Callers hand this a document `serde_json` has already accepted (for approval rule
/// template content, [`is_approval_rule_template_content`]); given text that
/// is not JSON it is still total, simply stripping whitespace outside quotes.
///
/// Two sibling services canonicalize a document of their own:
/// `service-com-amazonaws-rum-rum`'s `canonical_policy_document` scans the text the
/// same way, and `service-com-amazonaws-schemas-schemas`'s `canonical_json`
/// re-encodes through `serde_transcode`. The re-encode route is not used here
/// because, as that function's own doc comment records, it keeps a number's VALUE
/// but not its literal (`1e3` comes back `1000.0`), and this canonical form keeps
/// the literal a caller submitted.
pub fn canonical_json_document(document: &str) -> String {
    let mut canonical = String::with_capacity(document.len());
    // Where the scan is: inside a string literal, and whether the previous
    // character was the backslash of an escape (so an escaped quote does not close
    // the literal).
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            canonical.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                canonical.push(character);
            }
            // The four insignificant whitespace characters JSON defines. Everything
            // else outside a string literal is structure or a bare literal, and is
            // kept exactly as it came in.
            ' ' | '\t' | '\r' | '\n' => {}
            _ => canonical.push(character),
        }
    }
    canonical
}

/// Could `value` be a `RuleContentSha256` at all — the hex form [`content_sha256`]
/// reports, 64 hex characters for a 256-bit digest?
///
/// A value outside that form is not a signature of any content, which is what the
/// operations report as `InvalidRuleContentSha256Exception`.
pub fn is_content_sha256(value: &str) -> bool {
    value.len() == Sha256::output_size() * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The repository's opaque id: an RFC 9562 version-8 (custom) UUID whose 122
/// implementation-defined bits are a digest of the account, the repository name and
/// the creation instant. It is therefore stable for the life of the repository and
/// distinct for a repository of the same name created later.
fn repository_id(name: &str, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/{name}/{nanos}",
        ::engine::emulator_account_id()
    ))
}

/// An approval rule template's opaque `approvalRuleTemplateId`: the same kind of
/// version-8 UUID, over the account, the template name and the instant it was
/// created. It is therefore stable for the life of the template and distinct for a
/// template of the same name created later.
fn approval_rule_template_id(name: &str, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/approval-rule-template/{name}/{nanos}",
        ::engine::emulator_account_id()
    ))
}

/// Is this a syntactically valid comment id — the canonical 8-4-4-4-12 textual
/// form of the UUID this service mints a comment id as?
pub fn is_comment_id(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// Why a `commentId` cannot name a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentIdProblem {
    /// The id is absent or blank: the operations model `CommentIdRequiredException`
    /// for this.
    Missing,
    /// The id is not the form a comment id takes: `InvalidCommentIdException`.
    Invalid,
}

/// The comment id a request names, with surrounding space removed, or why it
/// names none. Every operation that takes a `commentId` classifies it here, so
/// they cannot disagree about which ids are addressable.
pub fn normalize_comment_id(raw: &str) -> Result<&str, CommentIdProblem> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CommentIdProblem::Missing);
    }
    if !is_comment_id(trimmed) {
        return Err(CommentIdProblem::Invalid);
    }
    Ok(trimmed)
}

/// The message `CommentDeletedException` carries: a FIXED sentence carrying no
/// value from the request.
///
/// CodeCommit answers a call against a comment whose content has been deleted with
/// "The specified comment has already been marked as deleted. Verify that you
/// specified a valid comment ID, and then try again." — the recorded answer to a
/// `GetComment` of such a comment is that sentence alone, so the sentence is what
/// the exception says and the four operations that reject a deleted comment cannot
/// disagree about it.
///
/// `refused_action` is the verb phrase of an operation the service is RECORDED
/// leading that sentence with ("You cannot delete this comment." for a repeated
/// `DeleteCommentContent`); an operation with no such recording passes `None`
/// rather than synthesising a lead clause of its own.
pub fn comment_deleted_message(refused_action: Option<&str>) -> String {
    // The sentences of this constant are separated by TWO spaces, which is part of
    // the recorded text rather than formatting latitude — hence the explicit
    // `concat!` rather than a line-continued literal, whose continuation would
    // swallow them.
    let already_deleted = concat!(
        "The specified comment has already been marked as deleted.",
        "  ",
        "Verify that you specified a valid comment ID, and then try again.",
    );
    match refused_action {
        Some(action) => format!("You cannot {action} this comment.  {already_deleted}"),
        None => already_deleted.to_string(),
    }
}

/// A comment's opaque `commentId`: the same kind of version-8 UUID, over the
/// account, the repository, the comment's post order and the instant it was
/// posted. It is therefore stable for the life of the comment and distinct from
/// every other comment's.
pub fn comment_id(repository: &str, sequence: u64, created: SystemTime) -> String {
    let nanos = nanos_since_epoch(created);
    uuid_v8(&format!(
        "codecommit/{}/{repository}/comment/{sequence}/{nanos}",
        ::engine::emulator_account_id()
    ))
}

fn nanos_since_epoch(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// An RFC 9562 version-8 (custom) UUID whose 122 implementation-defined bits are a
/// digest of `seed`, in the canonical 8-4-4-4-12 textual form.
fn uuid_v8(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 8 in the version nibble, and the RFC's variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Does this repository name carry the prohibited `.git` suffix?
///
/// CodeCommit documents it on every operation that takes a name for a repository
/// it is about to hold (`CreateRepository`, `UpdateRepositoryName`): "The suffix
/// .git is prohibited." Every other restriction on `RepositoryName` is the
/// modelled length and pattern, which the boundary validator refuses before a
/// handler runs, so this is the one name rule a handler is left to apply. Git
/// repository names are compared case-insensitively on this suffix, so `.GIT` is
/// prohibited too.
pub fn has_prohibited_git_suffix(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".git")
}

/// The widths of the dash-separated groups in the id KMS mints for a
/// single-region key: a UUID, written 8-4-4-4-12, as every KMS `KeyId` example
/// is (`1234abcd-12ab-34cd-56ef-1234567890ab`).
const KEY_ID_GROUP_WIDTHS: &[usize] = &[8, 4, 4, 4, 12];

/// The number of hexadecimal digits after the `mrk-` prefix of the id KMS mints
/// for a multi-region key.
const MULTI_REGION_KEY_ID_DIGITS: usize = 32;

/// Is this a KMS key id — hexadecimal digits grouped [`KEY_ID_GROUP_WIDTHS`] for
/// a single-region key, or `mrk-` and [`MULTI_REGION_KEY_ID_DIGITS`] of them for
/// a multi-region key?
///
/// This emulator resolves no KMS key, so only the FORM of the selector is
/// judged — but that form is the id's two shapes exactly, the same way
/// [`is_comment_id`] judges the UUID this service mints a comment id as.
fn is_kms_key_id(candidate: &str) -> bool {
    let hexadecimal = |group: &str| group.bytes().all(|byte| byte.is_ascii_hexdigit());
    if let Some(digits) = candidate.strip_prefix("mrk-") {
        return digits.len() == MULTI_REGION_KEY_ID_DIGITS && hexadecimal(digits);
    }
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == KEY_ID_GROUP_WIDTHS.len()
        && KEY_ID_GROUP_WIDTHS
            .iter()
            .zip(&groups)
            .all(|(width, group)| group.len() == *width && hexadecimal(group))
}

/// Is this a KMS alias path — `alias/` followed by a non-empty name of the
/// characters `KmsKeyId`'s modelled pattern `^[a-zA-Z0-9:/_-]+$` allows?
fn is_kms_alias(candidate: &str) -> bool {
    candidate.strip_prefix("alias/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/_-".contains(&byte))
    })
}

/// Is this the full ARN of a KMS key or alias
/// (`arn:<partition>:kms:<region>:<account>:key/<key id>` or `…:alias/<name>`)?
///
/// Read with the same six-field `splitn` this module's
/// [`repository_name_of_resource_arn`] uses, so one file has one way of reading an
/// ARN; this adds only which vendor and resource the value has to name.
fn is_kms_arn(candidate: &str) -> bool {
    let [prefix, partition, vendor, region, account, resource] =
        candidate.splitn(6, ':').collect::<Vec<_>>()[..]
    else {
        return false;
    };
    prefix == "arn"
        && !partition.is_empty()
        && vendor == "kms"
        && !region.is_empty()
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (is_kms_alias(resource) || resource.strip_prefix("key/").is_some_and(is_kms_key_id))
}

/// Does `key_id` select a KMS key at all?
///
/// `KmsKeyId`'s documentation sends a caller to KMS for the acceptable values —
/// "For more information about acceptable values for keyID, see KeyId in the
/// Decrypt API description in the Key Management Service API Reference"
/// (`smithy_query(query="doc",shape="UpdateRepositoryEncryptionKeyInput",member="kmsKeyId")`)
/// — and that description lists four forms: a key id, a key ARN, an alias name
/// and an alias ARN. The modelled pattern `^[a-zA-Z0-9:/_-]+$` admits text that
/// is none of them, so this is the rule a handler is left to apply; a value
/// outside every form names no key, and the operation answers the modelled
/// `EncryptionKeyInvalidIdException`.
pub fn is_kms_key_selector(key_id: &str) -> bool {
    is_kms_alias(key_id) || is_kms_key_id(key_id) || is_kms_arn(key_id)
}

/// The region this request is addressed to: the SigV4 credential scope's region,
/// falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The identity that made this request, as an ARN: what an operation reports as
/// the author of something the caller created.
///
/// Derived from the request's own SigV4 access key id, so a comment names the
/// caller it actually had rather than a fabricated one. The access key id is
/// caller-controlled text, so it is first reduced to the characters an access key
/// id can hold. Every request this emulator serves is signed with a caller's own
/// access key, so the identity is always an IAM identity of the emulator's
/// account.
pub fn caller_identity_arn(parts: &::http::request::Parts) -> String {
    let filtered: String = ::engine::extract_akid(&parts.headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// A `SystemTime` as the modeled timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// A git identity date: unix seconds plus a GMT offset, the form git (and so
/// CodeCommit's `UserInfo.date`) uses.
pub fn git_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs} +0000")
}

/// Is this a syntactically valid git object id (40 hex characters)?
pub fn is_object_id(candidate: &str) -> bool {
    git_object::is_object_id(candidate)
}

/// The longest reference name this service accepts.
///
/// `BranchName`'s modelled `length`
/// (`smithy_query(query="operation",operation="CreateBranch")` reports
/// `len 1..256`): a reference longer than this is one the control plane could not
/// address, so neither face of the service writes it.
pub const MAX_REF_NAME: usize = 256;

/// Can `refs/heads/<name>` be a git reference? CodeCommit is a git host, so a
/// branch name is a reference name: the rules below are git's own
/// (`git check-ref-format`), which is what makes a name a repository can hold
/// tellable from one it cannot.
///
/// The model constrains only the length of `BranchName`
/// (`smithy_query(query="operation",operation="CreateBranch")` reports
/// `len 1..256`), so every other malformed name arrives at a handler and is
/// refused there with `InvalidBranchNameException`. A caller that is not held to
/// the modelled length by the boundary validator — a git push, whose ref name is
/// not a modelled member — goes through [`is_valid_ref_name`], which adds it.
pub fn is_valid_branch_name(name: &str) -> bool {
    // git refuses a bare "@" as a reference name, a name that ends in "/" or
    // ".", the sequences "..", "//" and "@{", and every ASCII control
    // character as well as space ~ ^ : ? * [ and backslash.
    if name.is_empty() || name == "@" {
        return false;
    }
    if name.ends_with('/') || name.ends_with('.') {
        return false;
    }
    if name.contains("..") || name.contains("//") || name.contains("@{") {
        return false;
    }
    if name.bytes().any(|byte| {
        byte.is_ascii_control()
            || matches!(
                byte,
                b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'
            )
    }) {
        return false;
    }
    // No slash-separated component may be empty, start with "." or end in
    // ".lock" — the leading "/" case is the first component being empty.
    name.split('/').all(|component| {
        !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
    })
}

/// Can this be the short name of a reference a WRITE may create, in either
/// namespace (`refs/heads/<name>`, `refs/tags/<name>`)?
///
/// [`is_valid_branch_name`]'s rule — git's own `check-ref-format` — plus
/// [`MAX_REF_NAME`]. One implementation for both faces of the service: a name a
/// git push can create is exactly a name `CreateBranch` accepts, so the two
/// cannot disagree about which references a repository may hold. Smithy counts a
/// string's length in Unicode code points, as the boundary validator does for the
/// modelled member.
pub fn is_valid_ref_name(name: &str) -> bool {
    name.chars().count() <= MAX_REF_NAME && is_valid_branch_name(name)
}

/// The total content size of the folder a path lives in, over the sizes a tree
/// snapshot records for that folder's immediate entries.
pub fn folder_content_bytes(files: &BTreeMap<String, FileEntry>, path: &str) -> i64 {
    let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
    files
        .iter()
        .filter(|(other, _)| other.rsplit_once('/').map_or("", |(folder, _)| folder) == folder)
        .map(|(_, entry)| entry.size)
        .fold(0i64, i64::saturating_add)
}

/// The `blobId` of a file's bytes.
pub fn blob_id(content: &[u8]) -> String {
    git_object::object_id(Kind::Blob, content)
}

/// The git mode of a modeled `fileMode`: the numeric file mode permission code a
/// git tree entry carries, which is what `BlobMetadata.mode` reports (`100644`
/// read/write, `100755` read/write/execute, `120000` a symlink — see
/// `smithy_query(query="doc",shape="BlobMetadata",member="mode")`).
pub fn git_mode(file_mode: &str) -> &'static str {
    match file_mode {
        FILE_MODE_EXECUTABLE => git_object::MODE_EXECUTABLE,
        FILE_MODE_SYMLINK => git_object::MODE_SYMLINK,
        // NORMAL, and anything the enum does not declare, is a regular file.
        _ => git_object::MODE_FILE,
    }
}

/// The modeled `fileMode` a git tree entry mode names.
pub fn file_mode_of(mode: &str) -> &'static str {
    match mode {
        git_object::MODE_EXECUTABLE => FILE_MODE_EXECUTABLE,
        git_object::MODE_SYMLINK => FILE_MODE_SYMLINK,
        _ => FILE_MODE_NORMAL,
    }
}

/// The tree objects a whole snapshot is made of: the root's name, and every tree
/// object (root and nested) the snapshot needs, exactly as git would write them.
///
/// `None` when an entry's `blobId` is not an object name, which only a snapshot
/// built outside this module could carry: every writer here puts an id
/// [`Repository::insert_blob`] reported.
pub fn tree_objects(files: &BTreeMap<String, FileEntry>) -> Option<(String, Vec<Object>)> {
    let mut objects: Vec<Object> = Vec::new();
    let root = build_tree(files, &mut objects, 0)?;
    Some((root, objects))
}

/// The `treeId` of a whole tree snapshot, or `None` when an entry's `blobId` is
/// not an object name.
pub fn tree_id(files: &BTreeMap<String, FileEntry>) -> Option<String> {
    tree_objects(files).map(|(root, _)| root)
}

fn build_tree(
    files: &BTreeMap<String, FileEntry>,
    objects: &mut Vec<Object>,
    depth: usize,
) -> Option<String> {
    // One level of recursion per path segment, so the depth of this walk is bounded
    // where it happens — exactly as the opposite walk, [`Repository::flatten_tree_into`],
    // bounds itself. Every `filePath` a request supplies is already held to
    // [`MAX_PATH_SEGMENTS`] by [`normalize_path`], but this function is public and a
    // snapshot can also be assembled from a pushed packfile, so a snapshot whose keys
    // nest deeper than that is refused here rather than recursed into: a pathological
    // path cannot exhaust the stack (which in Rust aborts the process), it only fails
    // its own request.
    if depth >= MAX_PATH_SEGMENTS {
        return None;
    }
    let mut blobs: BTreeMap<&str, &FileEntry> = BTreeMap::new();
    let mut dirs: BTreeMap<&str, BTreeMap<String, FileEntry>> = BTreeMap::new();
    for (path, entry) in files {
        match path.split_once('/') {
            None => {
                blobs.insert(path.as_str(), entry);
            }
            Some((head, rest)) => {
                dirs.entry(head)
                    .or_default()
                    .insert(rest.to_string(), entry.clone());
            }
        }
    }

    let mut entries: Vec<TreeEntry> = Vec::new();
    for (name, entry) in blobs {
        entries.push(TreeEntry {
            mode: git_mode(&entry.file_mode).to_string(),
            name: name.to_string(),
            id: entry.blob_id.clone(),
        });
    }
    for (name, subtree) in &dirs {
        entries.push(TreeEntry {
            mode: git_object::MODE_TREE.to_string(),
            name: (*name).to_string(),
            id: build_tree(subtree, objects, depth + 1)?,
        });
    }
    let tree = git_object::encode_tree(entries)?;
    let id = tree.id();
    objects.push(tree);
    Some(id)
}

/// The `commitId` of a commit: the name of the git commit object over its tree,
/// parents, author, committer and message.
pub fn commit_id(
    tree: &str,
    parents: &[String],
    author: &UserInfo,
    committer: &UserInfo,
    message: Option<&str>,
) -> String {
    git_object::encode_commit(&git_object::CommitFields {
        tree: tree.to_string(),
        parents: parents.to_vec(),
        author: git_identity(author),
        committer: git_identity(committer),
        message: message.unwrap_or_default().to_string(),
    })
    .id()
}

fn git_identity(user: &UserInfo) -> String {
    git_object::join_identity(
        user.name.as_deref().unwrap_or(""),
        user.email.as_deref().unwrap_or(""),
        user.date.as_deref().unwrap_or("0 +0000"),
    )
}

/// Why a `filePath` cannot address a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathProblem {
    /// The path is absent, or empty once leading separators are removed: the
    /// operations model `PathRequiredException` for this.
    Missing,
    /// The path cannot name a file in a tree: `InvalidPathException`.
    Invalid,
}

/// The canonical form of a `filePath`: leading separators removed, so
/// `"/README.md"` and `"README.md"` address the same file, as they do in a git
/// tree where a path is always relative to the root.
pub fn normalize_path(raw: &str) -> Result<String, PathProblem> {
    let trimmed = raw.trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(PathProblem::Missing);
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() > MAX_PATH_SEGMENTS {
        return Err(PathProblem::Invalid);
    }
    // An empty, "." or ".." segment names no entry in a tree, and a NUL cannot
    // appear in a git tree entry name.
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == ".." )
        || trimmed.contains('\0')
    {
        return Err(PathProblem::Invalid);
    }
    Ok(trimmed.to_string())
}

/// Why a paginated list request cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageProblem {
    /// `maxResults` is outside the published range: the list operations model
    /// `InvalidMaxResultsException` for this. Carries the refusal message.
    InvalidMaxResults(String),
    /// `nextToken` is not a token this service issued: the list operations model
    /// `InvalidContinuationTokenException` for this. Carries the refusal message.
    InvalidContinuationToken(String),
}

/// One page of an ordered sequence of names, and the token that resumes after it.
///
/// The single pagination rule for this service's `maxResults`/`nextToken`
/// operations: `maxResults` must lie in [`MIN_MAX_RESULTS`]..=[`MAX_MAX_RESULTS`]
/// (absent means [`DEFAULT_MAX_RESULTS`]), `nextToken` must be a token this service
/// issued FOR THIS LISTING (see [`page_token`]) that still addresses a position in
/// the sequence, and the answer carries a token only when a further page exists.
///
/// `scope` names the listing the token belongs to — the operation, plus whatever
/// else picks the sequence out (the repository a per-repository listing reads), so
/// a token issued for one listing is not a token for another.
pub fn page_of(
    scope: &str,
    ordered: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<String>, Option<String>), PageProblem> {
    let limit = match max_results {
        None => DEFAULT_MAX_RESULTS,
        Some(requested) if (MIN_MAX_RESULTS..=MAX_MAX_RESULTS).contains(&requested) => requested,
        // `InvalidMaxResultsException`'s message comes from the one place this crate
        // words that code: the accepted range, with nothing from the request.
        Some(_) => {
            return Err(PageProblem::InvalidMaxResults(invalid_max_results_message(
                MIN_MAX_RESULTS,
                MAX_MAX_RESULTS,
            )));
        }
    };
    let start = match next_token {
        None => 0usize,
        Some(token) => page_offset(scope, token)
            // A token this service issued always names a position the sequence
            // still holds: one is only ever issued while a further page exists.
            .filter(|offset| *offset < ordered.len())
            .ok_or_else(|| {
                // `InvalidContinuationTokenException`'s message comes from the one
                // place this crate words that code: the template whose slot is the
                // token the request sent.
                PageProblem::InvalidContinuationToken(invalid_continuation_token_message(token))
            })?,
    };
    let end = ordered
        .len()
        .min(start.saturating_add(usize::try_from(limit).unwrap_or(0)));
    Ok((
        ordered[start..end].to_vec(),
        (end < ordered.len()).then(|| page_token(scope, end)),
    ))
}

/// The marker every continuation token this service issues carries, so a decimal
/// offset a caller invents is not mistaken for a token the service handed out.
const PAGE_TOKEN_MARKER: &str = "codecommit-page:";

/// The continuation token that resumes the `scope` listing at `offset`.
///
/// Hex-encoded so the token is opaque to the caller and carries no structure a
/// caller can act on. It is not integrity-protected and claims nothing: a caller
/// that reverses it can only name a position in the SAME listing, which is what
/// [`page_of`] already bounds.
fn page_token(scope: &str, offset: usize) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_MARKER}{scope}:{offset}"))
}

/// The position a continuation token names in the `scope` listing, or `None` for
/// anything this service did not issue for that listing. Every step is fallible on
/// purpose: the token is caller-supplied.
fn page_offset(scope: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(&format!("{PAGE_TOKEN_MARKER}{scope}:"))?
        .parse::<usize>()
        .ok()
}

/// The approval-rule-template content schema version CodeCommit publishes for the
/// `approvalRuleTemplateContent` document (the `Version` member of every template
/// body in the CodeCommit User Guide's approval rule template examples).
pub const APPROVAL_RULE_TEMPLATE_VERSION: &str = "2018-11-08";

/// The only `Statements[].Type` CodeCommit's approval rule template schema defines.
pub const APPROVAL_RULE_STATEMENT_TYPE: &str = "Approvers";

/// Is this `approvalRuleTemplateContent` a valid approval rule document?
///
/// CodeCommit's template content is a JSON document, not free text: an object with
/// a `Version` string, a non-empty `Statements` array whose every entry is an
/// `Approvers` statement carrying a positive `NumberOfApprovalsNeeded` and, where
/// present, an `ApprovalPoolMembers` array of strings, plus an optional
/// `DestinationReferences` array of strings (the structure the CodeCommit User
/// Guide documents for an approval rule template).
///
/// The answer is a BOOLEAN, not a reason: the operations that report a rejected
/// document answer [`INVALID_APPROVAL_RULE_CONTENT_MESSAGE`], and the one diagnostic
/// a caller must never be handed is the JSON reader's own ("key must be a string at
/// line 1 column 3"), whose offsets belong to whichever deserializer this emulator
/// links. Returning nothing to interpolate is what keeps it out of the wire.
pub fn is_approval_rule_template_content(content: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(content) else {
        return false;
    };
    let Some(object) = document.as_object() else {
        return false;
    };

    if object.get("Version").and_then(::serde_json::Value::as_str)
        != Some(APPROVAL_RULE_TEMPLATE_VERSION)
    {
        return false;
    }

    let Some(statements) = object
        .get("Statements")
        .and_then(::serde_json::Value::as_array)
    else {
        return false;
    };
    if statements.is_empty() {
        return false;
    }
    for statement in statements {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        if statement.get("Type").and_then(::serde_json::Value::as_str)
            != Some(APPROVAL_RULE_STATEMENT_TYPE)
        {
            return false;
        }
        match statement
            .get("NumberOfApprovalsNeeded")
            .and_then(::serde_json::Value::as_i64)
        {
            Some(needed) if needed >= 1 => {}
            _ => return false,
        }
        if let Some(members) = statement.get("ApprovalPoolMembers") {
            match members.as_array() {
                Some(members) if members.iter().all(::serde_json::Value::is_string) => {}
                _ => return false,
            }
        }
    }

    if let Some(references) = object.get("DestinationReferences") {
        match references.as_array() {
            Some(references) if references.iter().all(::serde_json::Value::is_string) => {}
            _ => return false,
        }
    }
    true
}

/// The canonical form of a `folderPath`: leading and trailing separators removed,
/// so `"/src/"`, `"/src"` and `"src"` all address the same folder, and an empty
/// path (or `"/"`) addresses the repository root — the tree itself, which always
/// exists in a commit. Every other form is checked exactly as a file path is, so
/// a folder and a file in it are addressed by the same path grammar.
pub fn normalize_folder_path(raw: &str) -> Result<String, PathProblem> {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        // The root folder: the whole tree, named by no segment at all.
        return Ok(String::new());
    }
    normalize_path(trimmed)
}

/// The entries of a tree snapshot that live under `folder`, keyed by their path
/// RELATIVE to that folder. `folder` is `""` for the repository root, whose
/// relative paths are the snapshot's own paths.
///
/// The result is itself a tree snapshot, so [`tree_id`] over it is the git tree
/// object name of that folder, and calling this again with one of its own folder
/// names descends another level.
pub fn subtree(files: &BTreeMap<String, FileEntry>, folder: &str) -> BTreeMap<String, FileEntry> {
    if folder.is_empty() {
        return files.clone();
    }
    let prefix = format!("{folder}/");
    files
        .iter()
        .filter_map(|(path, entry)| {
            path.strip_prefix(&prefix)
                .map(|relative| (relative.to_string(), entry.clone()))
        })
        .collect()
}

/// Every folder a path lives under, from its immediate parent outwards:
/// `"a/b/c.txt"` lives under `"a/b"` and `"a"`. Empty for a path at the root of
/// the tree, which is not a folder a deletion can empty.
pub fn ancestor_folders(path: &str) -> Vec<&str> {
    let mut folders = Vec::new();
    let mut rest = path;
    while let Some((folder, _)) = rest.rsplit_once('/') {
        folders.push(folder);
        rest = folder;
    }
    folders
}

/// The tree entry the kept-folder placeholder is stored as: the EMPTY blob, in
/// `NORMAL` mode. The caller stores that blob ([`Repository::insert_blob`]) so the
/// snapshot's entry names an object the store holds.
pub fn keep_empty_folders_entry() -> FileEntry {
    FileEntry {
        blob_id: blob_id(&[]),
        file_mode: FILE_MODE_NORMAL.to_string(),
        size: 0,
    }
}

/// Keep the folder structure a commit's deletions emptied, as `keepEmptyFolders`
/// asks: "If a file is the only object in the folder or directory, specifies
/// whether to delete the folder or directory that contains the file. By default,
/// empty folders are deleted" — and, for `CreateCommit`, "If true, a .gitkeep file
/// is created for empty folders."
///
/// Nothing else recreates a folder: a git tree holds no empty subtree. A deletion
/// can empty a whole chain of folders (`"a/b/c.txt"` empties `"a/b"` and then
/// `"a"`), so every ANCESTOR folder of a deleted path is considered, deepest
/// first: the placeholder written in the deepest emptied folder is what keeps each
/// folder above it non-empty, so the whole structure is kept with one placeholder.
///
/// Shared by every operation that takes `keepEmptyFolders` over a tree snapshot,
/// so no two of them can disagree about which folders a deletion keeps. Returns
/// the placeholder paths written, each stored as [`keep_empty_folders_entry`].
pub fn keep_empty_folders<'a>(
    files: &mut BTreeMap<String, FileEntry>,
    deleted_paths: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut candidates: Vec<&str> = deleted_paths
        .into_iter()
        .flat_map(ancestor_folders)
        .collect();
    candidates
        .sort_unstable_by_key(|folder| (std::cmp::Reverse(folder.matches('/').count()), *folder));
    candidates.dedup();
    let mut written: Vec<String> = Vec::new();
    for folder in candidates {
        let prefix = format!("{folder}/");
        if files.keys().any(|path| path.starts_with(&prefix)) {
            continue;
        }
        let path = format!("{prefix}{KEEP_EMPTY_FOLDERS_PLACEHOLDER}");
        files.insert(path.clone(), keep_empty_folders_entry());
        written.push(path);
    }
    written
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodecommitCodecommit20150413State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodecommitCodecommit20150413State>)
}
