// SPDX-License-Identifier: CC0-1.0

//! What a caller-supplied AMAZON S3 BUCKET NAME has to be.
//!
//! Several emulated services take a bucket name as an input member and then point a
//! resource of their own at it — a trail's log-delivery destination, a data feed, an
//! export, an import source. The value is therefore an instruction to write into or
//! read from a bucket, so it has to be a name Amazon S3 can hold a bucket under: a name
//! outside the published naming rules names no bucket at all, anywhere, so no amount of
//! provisioning makes such a request servable.
//!
//! Amazon S3 publishes one set of rules for every such name — the *General purpose
//! bucket naming rules* of the Amazon S3 User Guide, each sentence of which is one rule
//! in [`is_well_formed`] — and the services that bind such a member typically declare it
//! as a bare string with no `@length` or `@pattern`, so the framework's constraint walk
//! cannot refuse any of it. The rule lives here, once, so no two services disagree about
//! which names S3 holds; each service keeps only the envelope its own API refuses with
//! (the status, the error shape and the wording).
//!
//! Two services still carry their own copy of this reading, both written before this
//! module existed and each pinned by its own suite: `service-com-amazonaws-ec2-amazonec2`
//! (`state::s3_bucket_name_is_well_formed`, with the same rules and the same published
//! prefixes and suffixes) and the Amazon S3 crate's own `service::bucket_name_shaped`,
//! which is the narrower SHAPE test its host-addressing seam needs rather than the whole
//! rule. They are to be MIGRATED onto this module rather than copied again; nothing new
//! should restate the rule — call this module.

/// The shortest name a general-purpose bucket may have: "Bucket names must be between 3
/// (min) and 63 (max) characters long".
pub const MIN_BUCKET_NAME_CHARS: usize = 3;

/// The longest name a general-purpose bucket may have; see [`MIN_BUCKET_NAME_CHARS`].
pub const MAX_BUCKET_NAME_CHARS: usize = 63;

/// The prefixes a general-purpose bucket name must not start with: "Bucket names must not
/// start with the prefix `xn--`. … must not start with the prefix `sthree-`. … must not
/// start with the prefix `amzn-s3-demo-`".
pub const RESERVED_PREFIXES: [&str; 3] = ["xn--", "sthree-", "amzn-s3-demo-"];

/// The suffixes a general-purpose bucket name must not end with: "Bucket names must not
/// end with the suffix `-s3alias`. … must not end with the suffix `--ol-s3`. … must not
/// end with the suffix `.mrap`. … must not end with the suffix `--x-s3`. … must not end
/// with the suffix `--table-s3`".
pub const RESERVED_SUFFIXES: [&str; 5] = ["-s3alias", "--ol-s3", ".mrap", "--x-s3", "--table-s3"];

/// Whether `name` is a name Amazon S3 holds a general-purpose bucket under.
///
/// Every test below is one sentence of the published rules: the length
/// ([`MIN_BUCKET_NAME_CHARS`]..=[`MAX_BUCKET_NAME_CHARS`]), "can consist only of
/// lowercase letters, numbers, dots (.), and hyphens (-)", "must begin and end with a
/// letter or number", "must not contain two adjacent periods", "must not be formatted as
/// an IP address (for example, 192.168.5.4)", and the reserved prefixes and suffixes
/// above.
pub fn is_well_formed(name: &str) -> bool {
    // The character set FIRST, so the length rule below counts characters of a name that
    // can only be ASCII and a byte count is that character count.
    if !name.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
    }) {
        return false;
    }
    if !(MIN_BUCKET_NAME_CHARS..=MAX_BUCKET_NAME_CHARS).contains(&name.len()) {
        return false;
    }
    let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !(name.bytes().next().is_some_and(alphanumeric)
        && name.bytes().next_back().is_some_and(alphanumeric))
    {
        return false;
    }
    if name.contains("..") {
        return false;
    }
    // "Must not be formatted as an IP address": four dot-separated decimal octets.
    if name.parse::<::std::net::Ipv4Addr>().is_ok() {
        return false;
    }
    if RESERVED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    !RESERVED_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_rules_admit_a_bucket_name_and_nothing_else() {
        for name in [
            "abc",
            "cloudtrail-logs-probe",
            "logs.cloudtrail.probe",
            "20261030123456",
            "a-b.c-d",
            &"x".repeat(MAX_BUCKET_NAME_CHARS),
        ] {
            assert!(
                is_well_formed(name),
                "case {name:?}: S3 holds a bucket under this name, got rejected"
            );
        }
        for name in [
            "",
            "ab",
            &"x".repeat(MAX_BUCKET_NAME_CHARS + 1),
            "Invalid_Bucket_Name_!!",
            "UPPERCASE",
            "under_score",
            "space name",
            "-leading-hyphen",
            "trailing-hyphen-",
            ".leading-dot",
            "trailing-dot.",
            "two..dots",
            "192.168.5.4",
            "xn--bucket",
            "sthree-bucket",
            "amzn-s3-demo-bucket",
            "bucket-s3alias",
            "bucket--ol-s3",
            "bucket.mrap",
            "bucket--x-s3",
            "bucket--table-s3",
        ] {
            assert!(
                !is_well_formed(name),
                "case {name:?}: S3 holds no bucket under this name, got accepted"
            );
        }
    }

    #[test]
    fn the_length_bound_holds_at_each_published_edge() {
        for (length, acceptable) in [
            (MIN_BUCKET_NAME_CHARS - 1, false),
            (MIN_BUCKET_NAME_CHARS, true),
            (MIN_BUCKET_NAME_CHARS + 1, true),
            (MAX_BUCKET_NAME_CHARS - 1, true),
            (MAX_BUCKET_NAME_CHARS, true),
            (MAX_BUCKET_NAME_CHARS + 1, false),
        ] {
            let name = "b".repeat(length);
            assert_eq!(
                is_well_formed(&name),
                acceptable,
                "case {length} characters: expected acceptable={acceptable}, got {}",
                is_well_formed(&name)
            );
        }
    }
}
