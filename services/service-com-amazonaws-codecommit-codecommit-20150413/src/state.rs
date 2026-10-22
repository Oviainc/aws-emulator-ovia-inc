// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codecommit-codecommit-20150413 service.
//!
//! The store is a map of live repositories, each carrying its own branches,
//! commits, blobs and the comments posted on its commit comparisons, so every
//! response an operation returns is a lookup in state rather than a value made up
//! on the request path.
//!
//! Object ids are content-addressed over the git object encoding: `blobId`, `treeId`
//! and `commitId` hash exactly what git hashes for that object
//! (`"<kind> <len>\0<payload>"`, with real nested tree objects and git mode bits),
//! because CodeCommit is a git host and reports git object names. The digest is
//! SHA-256 truncated to the leading 160 bits, so an id is 40 hex characters wide,
//! identical content yields the identical id, and changing a parent, tree, author or
//! message changes the commit id — it is simply not byte-identical to the SHA-1 a git
//! client would compute for the same object.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{Comment, Location, RepositoryMetadata, UserInfo};

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
/// bounds the recursion in [`tree_id`], which walks one level per path segment,
/// so a pathological path cannot exhaust the stack.
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

/// How many files one repository's tree holds before `PutFile` refuses the write.
///
/// The emulator's OWN bound, not an AWS limit: a commit stores the whole tree
/// snapshot, so this is what keeps both the per-request work ([`tree_id`] hashes
/// that snapshot) and one repository's stored size bounded.
pub const MAX_FILES_PER_TREE: usize = 1_000;

/// How many commits one repository retains. Older commits that are not the tip of
/// a branch are dropped by [`Repository::prune_history`], together with the blobs
/// only they referenced, so repeated writes cannot grow the store without end.
///
/// The emulator's OWN bound, not an AWS limit: CodeCommit keeps all history. A
/// read at a commit that has been dropped answers `CommitDoesNotExistException`,
/// the same as a commit id the repository never held. With
/// [`MAX_FILES_PER_TREE`] it bounds the product of commits and files per commit,
/// which is what one repository can cost.
pub const MAX_COMMITS_RETAINED: usize = 256;

/// How many branches one repository holds before `CreateBranch` refuses the write.
///
/// The emulator's OWN bound, not an AWS limit: a branch is a caller-named
/// reference kept for the life of the repository, and
/// [`Repository::prune_history`] never drops a branch tip, so an unbounded number
/// of branches would pin an unbounded number of commits and defeat
/// [`MAX_COMMITS_RETAINED`]. It is therefore deliberately well BELOW that bound,
/// which is what lets pruning always reach it. Every record it holds is released
/// by `DeleteRepository`.
pub const MAX_BRANCHES_PER_REPOSITORY: usize = 64;

/// The largest total content size of one folder, in bytes. CodeCommit documents
/// this limit on `FolderContentSizeLimitExceededException`: a commit cannot be
/// created when a change "results in a folder whose contents exceed the limit of
/// 6 MB".
pub const MAX_FOLDER_CONTENT_BYTES: i64 = 6 * 1024 * 1024;

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
/// [`MAX_COMMITS_RETAINED`] commits, [`MAX_BRANCHES_PER_REPOSITORY`] branches and
/// [`MAX_COMMENTS_PER_REPOSITORY`] comments, with blobs released as soon as no
/// retained commit references them.
#[derive(Debug, Default)]
pub struct ComAmazonawsCodecommitCodecommit20150413State {
    /// Live repositories, keyed by `repositoryName` (unique per account).
    pub repositories: BTreeMap<String, Repository>,
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
    pub clone_url_http: String,
    pub clone_url_ssh: String,
    pub kms_key_id: Option<String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    pub branches: BTreeMap<String, Branch>,
    pub commits: BTreeMap<String, Commit>,
    pub blobs: BTreeMap<String, Vec<u8>>,
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
            clone_url_http: clone_url_http(region, name),
            clone_url_ssh: clone_url_ssh(region, name),
            kms_key_id,
            creation_date: created,
            last_modified_date: created,
            tags,
            branches: BTreeMap::new(),
            commits: BTreeMap::new(),
            blobs: BTreeMap::new(),
            next_commit_sequence: 0,
            comments: BTreeMap::new(),
            comment_tokens: BTreeMap::new(),
            next_comment_sequence: 0,
        }
    }

    /// This repository as the modeled `RepositoryMetadata` shape — the one place
    /// that projection is written, so `CreateRepository` and `GetRepository`
    /// cannot disagree about a stored repository.
    pub fn metadata(&self) -> RepositoryMetadata {
        RepositoryMetadata {
            account_id: Some(self.account_id.clone()),
            repository_id: Some(self.repository_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_description: self.description.clone(),
            default_branch: self.default_branch.clone(),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            clone_url_http: Some(self.clone_url_http.clone()),
            clone_url_ssh: Some(self.clone_url_ssh.clone()),
            arn: Some(self.arn.clone()),
            kms_key_id: self.kms_key_id.clone(),
        }
    }

    /// Rename this repository, recomputing every field CodeCommit derives from the
    /// repository name — the ARN and both clone URLs address a repository BY name —
    /// and moving the last-modified instant to the rename.
    ///
    /// Everything that belongs to the resource rather than to its name (the
    /// repository id, the creation date, the description, the KMS key, the tags, the
    /// branches, the commits and the blobs) carries over untouched, so a renamed
    /// repository is the same repository under a new name. The caller re-keys the
    /// store, which is keyed by `repositoryName`.
    pub fn rename(&mut self, new_name: &str, renamed: SystemTime) {
        self.name = new_name.to_string();
        self.arn = ::engine::emulator_arn("codecommit", &self.region, new_name);
        self.clone_url_http = clone_url_http(&self.region, new_name);
        self.clone_url_ssh = clone_url_ssh(&self.region, new_name);
        self.last_modified_date = renamed;
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

    /// Every commit id reachable from `commit_id` by following parents, including
    /// `commit_id` itself: the history a read at that commit sees. An id whose
    /// commit [`Repository::prune_history`] has dropped is still reported, because
    /// a retained commit's `parents` keeps naming it.
    pub fn ancestry(&self, commit_id: &str) -> std::collections::BTreeSet<String> {
        let mut reachable = std::collections::BTreeSet::new();
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

    /// Drop the oldest commits beyond [`MAX_COMMITS_RETAINED`], then every blob no
    /// retained commit references. A branch tip is never dropped, so every branch
    /// stays readable; an older commit id becomes one the repository no longer
    /// holds.
    pub fn prune_history(&mut self) {
        let tips: std::collections::BTreeSet<String> = self
            .branches
            .values()
            .map(|branch| branch.commit_id.clone())
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
        let referenced: std::collections::BTreeSet<String> = self
            .commits
            .values()
            .flat_map(|commit| commit.files.values().map(|entry| entry.blob_id.clone()))
            .collect();
        self.blobs.retain(|blob_id, _| referenced.contains(blob_id));
    }
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

/// The HTTPS clone URL CodeCommit publishes for a repository in a region.
pub fn clone_url_http(region: &str, name: &str) -> String {
    format!("https://git-codecommit.{region}.amazonaws.com/v1/repos/{name}")
}

/// The SSH clone URL CodeCommit publishes for a repository in a region.
pub fn clone_url_ssh(region: &str, name: &str) -> String {
    format!("ssh://git-codecommit.{region}.amazonaws.com/v1/repos/{name}")
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

/// The width of an object id in bytes: a git object name is 20 bytes, written as 40
/// hex characters.
const OBJECT_ID_BYTES: usize = 20;

/// Is this a syntactically valid git object id (40 hex characters)?
pub fn is_object_id(candidate: &str) -> bool {
    candidate.len() == 2 * OBJECT_ID_BYTES && candidate.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Can `refs/heads/<name>` be a git reference? CodeCommit is a git host, so a
/// branch name is a reference name: the rules below are git's own
/// (`git check-ref-format`), which is what makes a name a repository can hold
/// tellable from one it cannot.
///
/// The model constrains only the length of `BranchName`
/// (`smithy_query(query="operation",operation="CreateBranch")` reports
/// `len 1..256`), so every other malformed name arrives at a handler and is
/// refused there with `InvalidBranchNameException`.
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

/// The id of a git object: the git object encoding `"<kind> <len>\0"` plus the
/// payload, hashed and reported as the 40 hex characters a git object name is wide
/// (see this module's header for how the digest is formed).
fn git_object_id(kind: &str, payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{kind} {}\0", payload.len()).as_bytes());
    hasher.update(payload);
    ::hex::encode(&hasher.finalize()[..OBJECT_ID_BYTES])
}

/// The `blobId` of a file's bytes.
pub fn blob_id(content: &[u8]) -> String {
    git_object_id("blob", content)
}

/// The git mode of a modeled `fileMode`: the numeric file mode permission code a
/// git tree entry carries, which is what `BlobMetadata.mode` reports (`100644`
/// read/write, `100755` read/write/execute, `120000` a symlink — see
/// `smithy_query(query="doc",shape="BlobMetadata",member="mode")`).
pub fn git_mode(file_mode: &str) -> &'static str {
    match file_mode {
        FILE_MODE_EXECUTABLE => "100755",
        FILE_MODE_SYMLINK => "120000",
        // NORMAL, and anything the enum does not declare, is a regular file.
        _ => "100644",
    }
}

/// The `treeId` of a whole tree snapshot: the git tree object over the entries,
/// recursing into one subtree per path prefix exactly as git does.
pub fn tree_id(files: &BTreeMap<String, FileEntry>) -> String {
    let mut payload: Vec<u8> = Vec::new();
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

    // git orders tree entries by name, comparing a subtree's name as if it ended
    // in '/', which is what puts "a.txt" before "a/b" in a tree object.
    let mut entries: Vec<(String, &'static str, String, String)> = Vec::new();
    for (name, entry) in blobs {
        entries.push((
            name.to_string(),
            git_mode(&entry.file_mode),
            name.to_string(),
            entry.blob_id.clone(),
        ));
    }
    for (name, subtree) in &dirs {
        entries.push((
            format!("{name}/"),
            "40000",
            (*name).to_string(),
            tree_id(subtree),
        ));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    for (_, mode, name, id) in entries {
        payload.extend_from_slice(format!("{mode} {name}\0").as_bytes());
        payload.extend_from_slice(&::hex::decode(&id).unwrap_or_default());
    }
    git_object_id("tree", &payload)
}

/// The `commitId` of a commit: the git commit object over its tree, parents,
/// author, committer and message.
pub fn commit_id(
    tree: &str,
    parents: &[String],
    author: &UserInfo,
    committer: &UserInfo,
    message: Option<&str>,
) -> String {
    let mut payload = format!("tree {tree}\n");
    for parent in parents {
        payload.push_str(&format!("parent {parent}\n"));
    }
    payload.push_str(&format!("author {}\n", git_identity(author)));
    payload.push_str(&format!("committer {}\n", git_identity(committer)));
    payload.push('\n');
    if let Some(message) = message {
        payload.push_str(message);
    }
    git_object_id("commit", payload.as_bytes())
}

fn git_identity(user: &UserInfo) -> String {
    format!(
        "{} <{}> {}",
        user.name.as_deref().unwrap_or(""),
        user.email.as_deref().unwrap_or(""),
        user.date.as_deref().unwrap_or("0 +0000")
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
