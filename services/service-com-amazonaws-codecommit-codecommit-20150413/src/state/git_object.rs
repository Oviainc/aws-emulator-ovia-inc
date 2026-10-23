// SPDX-License-Identifier: CC0-1.0

//! Git objects: the four object kinds, the object NAME function, and the
//! encodings CodeCommit's two faces share.
//!
//! CodeCommit is a git host. The ids it reports through the control plane
//! (`blobId`, `treeId`, `commitId`) are git object names, and the bytes it serves
//! over the git smart-HTTP transport are git objects. Both come from this one
//! module, so an id a response reports is the name of the object the transport
//! serves for it, and an object a client pushes keeps the name the client
//! computed — which is what lets a `git clone` of API-written content and a
//! `GetFile` of pushed content both work.
//!
//! An object name is `SHA-1("<kind> <len>\0" + payload)`: that is git's object
//! name function, so the digest choice is dictated by the wire format a git
//! client speaks (git's SHA-256 object format is opt-in per repository and no
//! client negotiates it here). It authenticates nothing and guards nothing; it
//! is a content address.

use sha1::{Digest, Sha1};

/// Width of an object name in bytes. A name is written as twice this many hex
/// characters.
pub const OBJECT_ID_BYTES: usize = 20;

/// The all-zero object name, which the git transport uses to mean "no object"
/// (the old value of a ref being created, or the new value of one being
/// deleted).
pub const NULL_OBJECT_ID: &str = "0000000000000000000000000000000000000000";

/// The kinds of object a git repository holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A commit: a tree, its parents and the two identities.
    Commit,
    /// A tree: one directory's entries.
    Tree,
    /// A blob: a file's bytes.
    Blob,
    /// An annotated tag: a named, described pointer at another object.
    Tag,
}

impl Kind {
    /// The kind's name as it appears in the object header and on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Tree => "tree",
            Self::Blob => "blob",
            Self::Tag => "tag",
        }
    }

    /// The kind a header name or a `want`-side type string names.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "commit" => Some(Self::Commit),
            "tree" => Some(Self::Tree),
            "blob" => Some(Self::Blob),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }

    /// The kind's type number in a packfile entry header.
    pub fn pack_type(self) -> u8 {
        match self {
            Self::Commit => 1,
            Self::Tree => 2,
            Self::Blob => 3,
            Self::Tag => 4,
        }
    }

    /// The kind a packfile entry header's type number names, or `None` for a
    /// delta type (6, 7) or a number no kind uses.
    pub fn from_pack_type(pack_type: u8) -> Option<Self> {
        match pack_type {
            1 => Some(Self::Commit),
            2 => Some(Self::Tree),
            3 => Some(Self::Blob),
            4 => Some(Self::Tag),
            _ => None,
        }
    }
}

/// One stored git object: its kind and its payload (the bytes after the
/// `"<kind> <len>\0"` header, which is the form the transport compresses and the
/// name function hashes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub kind: Kind,
    pub data: Vec<u8>,
}

impl Object {
    pub fn new(kind: Kind, data: Vec<u8>) -> Self {
        Self { kind, data }
    }

    /// This object's name.
    pub fn id(&self) -> String {
        object_id(self.kind, &self.data)
    }
}

/// The name of an object of `kind` with `data` as its payload.
pub fn object_id(kind: Kind, data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{} {}\0", kind.name(), data.len()).as_bytes());
    hasher.update(data);
    ::hex::encode(hasher.finalize())
}

/// The SHA-1 digest of `data`, the checksum a packfile carries in its trailer
/// and the digest an object name is formed with.
pub fn sha1_digest(data: &[u8]) -> [u8; OBJECT_ID_BYTES] {
    let mut digest = [0u8; OBJECT_ID_BYTES];
    digest.copy_from_slice(&Sha1::digest(data));
    digest
}

/// Is this a syntactically valid object name (40 hex characters)?
pub fn is_object_id(candidate: &str) -> bool {
    candidate.len() == 2 * OBJECT_ID_BYTES && candidate.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Is this the all-zero object name?
pub fn is_null_object_id(candidate: &str) -> bool {
    candidate.len() == NULL_OBJECT_ID.len() && candidate.bytes().all(|b| b == b'0')
}

/// The git mode of a subtree entry. Git writes it without the leading zero
/// (`40000`, not `040000`), which is what the tree object's name is computed
/// over.
pub const MODE_TREE: &str = "40000";
/// A regular, non-executable file.
pub const MODE_FILE: &str = "100644";
/// A regular, executable file.
pub const MODE_EXECUTABLE: &str = "100755";
/// A symbolic link; the blob holds the link target.
pub const MODE_SYMLINK: &str = "120000";
/// A gitlink: a commit embedded in a tree by a submodule. Not a file, so it
/// names no blob.
pub const MODE_GITLINK: &str = "160000";

/// One entry of a tree object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: String,
    pub name: String,
    pub id: String,
}

impl TreeEntry {
    /// Does this entry name a subtree?
    pub fn is_tree(&self) -> bool {
        self.mode == MODE_TREE || self.mode == "040000"
    }

    /// The key git sorts a tree's entries by: the entry name, with a subtree
    /// compared as though its name ended in `/` (which is what puts `a.txt`
    /// before `a/b` in a tree object).
    fn sort_key(&self) -> String {
        if self.is_tree() {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// The tree object over `entries`, in git's tree entry order, or `None` when an
/// entry does not name an object.
///
/// An id that is not an object name cannot be encoded: a tree entry carries the
/// raw 20 bytes of one, so a short or malformed id would silently produce a tree
/// whose bytes — and therefore whose own name — are wrong. It is refused here
/// instead.
pub fn encode_tree(mut entries: Vec<TreeEntry>) -> Option<Object> {
    entries.sort_by_key(TreeEntry::sort_key);
    let mut data: Vec<u8> = Vec::new();
    for entry in entries {
        data.extend_from_slice(entry.mode.as_bytes());
        data.push(b' ');
        data.extend_from_slice(entry.name.as_bytes());
        data.push(0);
        let id = ::hex::decode(&entry.id).ok().filter(|id| id.len() == OBJECT_ID_BYTES)?;
        data.extend_from_slice(&id);
    }
    Some(Object::new(Kind::Tree, data))
}

/// The entries of a tree object, or `None` when the payload is not a tree.
pub fn decode_tree(data: &[u8]) -> Option<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|b| *b == b' ')?;
        let nul = rest.iter().position(|b| *b == 0)?;
        if nul < space {
            return None;
        }
        let mode = std::str::from_utf8(&rest[..space]).ok()?.to_string();
        let name = std::str::from_utf8(&rest[space + 1..nul]).ok()?.to_string();
        let id_start = nul + 1;
        let id_end = id_start.checked_add(OBJECT_ID_BYTES)?;
        let id = ::hex::encode(rest.get(id_start..id_end)?);
        entries.push(TreeEntry { mode, name, id });
        rest = &rest[id_end..];
    }
    Some(entries)
}

/// A commit object's fields, as the control plane projects them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFields {
    pub tree: String,
    pub parents: Vec<String>,
    /// The raw `author` identity line (`Name <email> <seconds> <offset>`).
    pub author: String,
    /// The raw `committer` identity line.
    pub committer: String,
    pub message: String,
}

/// The commit object for these fields, in git's commit encoding.
pub fn encode_commit(fields: &CommitFields) -> Object {
    let mut text = format!("tree {}\n", fields.tree);
    for parent in &fields.parents {
        text.push_str(&format!("parent {parent}\n"));
    }
    text.push_str(&format!("author {}\n", fields.author));
    text.push_str(&format!("committer {}\n", fields.committer));
    text.push('\n');
    text.push_str(&fields.message);
    Object::new(Kind::Commit, text.into_bytes())
}

/// The fields of a commit object, or `None` when the payload names no tree.
///
/// Headers this emulator does not project (`gpgsig`, `encoding`, …) are skipped
/// rather than refused: the object is stored and served back byte-for-byte, so
/// dropping them from the PROJECTION loses nothing a reader of this crate's
/// `Commit` record can see.
pub fn decode_commit(data: &[u8]) -> Option<CommitFields> {
    let (header, message) = split_headers(data);
    let mut fields = CommitFields {
        tree: String::new(),
        parents: Vec::new(),
        author: String::new(),
        committer: String::new(),
        message,
    };
    for (key, value) in header {
        match key.as_str() {
            "tree" => fields.tree = value,
            "parent" => fields.parents.push(value),
            "author" => fields.author = value,
            "committer" => fields.committer = value,
            _ => {}
        }
    }
    is_object_id(&fields.tree).then_some(fields)
}

/// An annotated tag object's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFields {
    /// The object the tag points at.
    pub object: String,
    /// The kind of that object.
    pub kind: Kind,
}

/// The object an annotated tag points at, or `None` when the payload is not a
/// tag object.
pub fn decode_tag(data: &[u8]) -> Option<TagFields> {
    let (header, _) = split_headers(data);
    let mut object = String::new();
    let mut kind = None;
    for (key, value) in header {
        match key.as_str() {
            "object" => object = value,
            "type" => kind = Kind::from_name(&value),
            _ => {}
        }
    }
    let kind = kind?;
    is_object_id(&object).then_some(TagFields { object, kind })
}

/// Split a commit or tag payload into its `key value` header lines and the
/// message that follows the first empty line. A continuation line (one starting
/// with a space, as a multi-line `gpgsig` uses) belongs to the header above it
/// and is skipped.
fn split_headers(data: &[u8]) -> (Vec<(String, String)>, String) {
    let text = String::from_utf8_lossy(data);
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut lines = text.split_inclusive('\n');
    let mut consumed = 0usize;
    for line in lines.by_ref() {
        consumed += line.len();
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed.is_empty() {
            break;
        }
        if trimmed.starts_with(' ') {
            continue;
        }
        let (key, value) = trimmed.split_once(' ').unwrap_or((trimmed, ""));
        headers.push((key.to_string(), value.to_string()));
    }
    (headers, text[consumed.min(text.len())..].to_string())
}

/// A git identity line split into its parts: the name, the email and the date
/// (`<seconds> <offset>`), which is the form CodeCommit's `UserInfo.date` uses.
pub fn split_identity(identity: &str) -> (String, String, String) {
    let (name, rest) = identity.split_once(" <").unwrap_or((identity, ""));
    let (email, date) = rest.split_once("> ").unwrap_or((rest, ""));
    (
        name.trim().to_string(),
        email.trim().to_string(),
        date.trim().to_string(),
    )
}

/// A git identity line built from its parts — the inverse of
/// [`split_identity`].
pub fn join_identity(name: &str, email: &str, date: &str) -> String {
    format!("{name} <{email}> {date}")
}
