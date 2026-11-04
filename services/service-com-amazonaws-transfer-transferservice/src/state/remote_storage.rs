// SPDX-License-Identifier: CC0-1.0

//! The remote filesystem a connector operation acts on: the object a path names, and
//! the containment that keeps a path inside the bucket it names.
//!
//! Every path a connector operation carries — the `SendFilePaths` of a transfer and the
//! `RemoteDirectoryPath` it lands under, the `SourcePath`/`TargetPath` of a move, the
//! `DeletePath` of a delete, a listing's `RemoteDirectoryPath` and
//! `OutputDirectoryPath` — addresses a file by its bucket and key:
//! "`amzn-s3-demo-bucket/myfile.txt`" (`StartFileTransfer`), and the same path with the
//! leading separator for a server's own filesystem, whose root holds one entry per
//! bucket. So ONE parse lives here, and every connector path of this service becomes an
//! object address through it ([`super::connector_delivery::split_storage_path`] reads
//! it too).
//!
//! NOTHING here constructs a host filesystem path. A caller-supplied path becomes a
//! bucket name and an object key and nothing else, and it is only ever handed to the
//! emulated Amazon S3 as those two values; a path that does not stay inside the bucket
//! it names — one of whose elements is a relative-traversal segment — resolves to
//! nothing, so the operation carrying it acts on nothing at all.

/// One object addressed by a connector path: the bucket its first element names and the
/// key the rest of it spells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPath {
    pub bucket: String,
    /// The key, which is EMPTY for a path that names a bucket and nothing in it (the
    /// root directory of that bucket).
    pub key: String,
}

impl ObjectPath {
    /// This object's path in the form a request writes it: `/<bucket>/<key>`.
    ///
    /// Absolute, because that is what a path on the remote server is
    /// ([`super::is_absolute_remote_path`]), and what a listing reports its entries by.
    pub fn absolute(&self) -> String {
        if self.key.is_empty() {
            format!("/{}", self.bucket)
        } else {
            format!("/{}/{}", self.bucket, self.key)
        }
    }

    /// The entry `name` inside this directory.
    pub fn child(&self, name: &str) -> Self {
        let key = if self.key.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.key.trim_end_matches('/'), name)
        };
        Self {
            bucket: self.bucket.clone(),
            key,
        }
    }

    /// The prefix every object UNDER this directory shares: its key with the one
    /// separator that follows a directory name, and nothing at all for a bucket root.
    pub fn prefix(&self) -> String {
        if self.key.is_empty() {
            String::new()
        } else {
            format!("{}/", self.key.trim_end_matches('/'))
        }
    }
}

/// The object a FILE path names — `None` when the path names no file.
///
/// A path is read as `[/]<bucket>/<key>`: the first element is the bucket and the rest
/// is the key, whichever side of a transfer the path belongs to, because a Transfer
/// Family file path is the same shape on both. A path with no key element names a
/// bucket rather than a file, so it is not a file path at all.
pub fn object_path(path: &str) -> Option<ObjectPath> {
    let resolved = directory_path(path)?;
    (!resolved.key.is_empty()).then_some(resolved)
}

/// The object a DIRECTORY path names — `None` when the path names no bucket, or when it
/// is not CONTAINED by the bucket it names.
///
/// Same parse as [`object_path`], except that a path naming a bucket alone is the root
/// directory of that bucket and is therefore a directory this answers.
///
/// CONTAINMENT: a key element that is a relative-traversal segment (`.` or `..`), or
/// that is empty, names no entry of the directory it sits in, so such a path resolves
/// to nothing here. A key with no elements at all is the bucket's root, which is inside
/// the bucket by definition.
pub fn directory_path(path: &str) -> Option<ObjectPath> {
    let trimmed = path.trim().trim_start_matches('/');
    let (bucket, key) = match trimmed.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (trimmed, ""),
    };
    if bucket.is_empty() || is_traversal(bucket) {
        return None;
    }
    let key = key.trim_matches('/');
    if !key.is_empty() && key.split('/').any(is_traversal) {
        return None;
    }
    Some(ObjectPath {
        bucket: bucket.to_string(),
        key: key.to_string(),
    })
}

/// Whether one element of a path is a relative-traversal segment — `.` or `..` — or is
/// empty, none of which names an entry of the directory it sits in.
fn is_traversal(element: &str) -> bool {
    element.is_empty() || element == "." || element == ".."
}

/// The file name a path ends in — its last element — `None` when it names none.
pub fn file_name(path: &str) -> Option<&str> {
    path.trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
}
