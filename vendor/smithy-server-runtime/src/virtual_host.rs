// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Virtual-hosted addressing: a bucket named in the `Host` instead of the path.
//!
//! A service whose model declares an endpoint rule set says, for each shape of address a
//! client can build, a URL template. For Amazon S3 the shapes that name the bucket in the
//! host are `{Bucket}.s3.{Region}.{dnsSuffix}`, `{Bucket}.s3.dualstack.{Region}.{dnsSuffix}`,
//! `{Bucket}.s3-fips...`, `{Bucket}.s3-accelerate...` and so on. `#[smithy_service]` reads
//! those templates off the model (it keeps the ones the operations' leading path label can
//! take, and the ones that need nothing but a Region and a partition DNS suffix) and hands
//! them to [`VirtualHostAddressing`], which recognizes a request's host against them. No
//! service's host rules are written here.
//!
//! Each template is the part of the host after the bucket, as dot-separated labels: a
//! literal label (`s3`), `{region}` (one label) or `{dnsSuffix}` (a partition's DNS suffix,
//! last).

/// The DNS suffix of each AWS partition. A partition's suffix is not part of a service
/// model (the endpoint rules ask the SDK's partition table for it), so it is listed here;
/// the S3 endpoint tests in `restxml-e2e-tests` fail if a suffix they expect is missing.
const PARTITION_DNS_SUFFIXES: &[&str] = &[
    "amazonaws.com",
    "amazonaws.com.cn",
    "amazonaws.eu",
    "c2s.ic.gov",
    "sc2s.sgov.gov",
    "cloud.adc-e.uk",
    "csp.hci.ic.gov",
];

/// A virtual-hosted address split in two: the bucket the host leads with, and the endpoint
/// host left once it is removed (with any `:port` the address carried).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualHost<'a> {
    /// The bucket, e.g. `my.bucket` in `my.bucket.s3.us-east-1.amazonaws.com`.
    pub bucket: &'a str,
    /// The path-style endpoint, e.g. `s3.us-east-1.amazonaws.com`.
    pub endpoint: &'a str,
}

/// The host templates of a service whose requests may name the bucket in the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualHostAddressing {
    templates: &'static [&'static str],
}

impl VirtualHostAddressing {
    /// `templates` are the host parts after the bucket, from the model's endpoint rules.
    pub const fn new(templates: &'static [&'static str]) -> Self {
        Self { templates }
    }

    /// The templates this was built from.
    pub const fn templates(&self) -> &'static [&'static str] {
        self.templates
    }

    /// The bucket and endpoint of a `Host` or `:authority`, or `None` when it is not a
    /// virtual-hosted address of this service.
    ///
    /// The host is untrusted input that the router splices into a URI, so only letters,
    /// digits, `.`, `-` and `_` (and a numeric `:port`) are accepted; anything else is not
    /// matched. A bucket may contain dots. When more than one template fits, the one that
    /// leaves the longest bucket wins, so the rightmost endpoint label ends the bucket.
    pub fn split<'a>(&self, authority: &'a str) -> Option<VirtualHost<'a>> {
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let (name, port) = match host.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        };
        if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        let name = name.strip_suffix('.').unwrap_or(name);
        let dns_like = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_');
        if name.is_empty() || !name.bytes().all(dns_like) {
            return None;
        }
        let labels: Vec<&str> = name.split('.').collect();
        if labels.iter().any(|label| label.is_empty()) {
            return None;
        }
        let bucket_labels = self
            .templates
            .iter()
            .filter_map(|template| bucket_labels_before(template, &labels))
            .max()?;
        let bucket_len: usize = labels[..bucket_labels]
            .iter()
            .map(|label| label.len() + 1)
            .sum();
        Some(VirtualHost {
            bucket: &host[..bucket_len - 1],
            endpoint: &host[bucket_len..],
        })
    }
}

/// How many leading labels of `labels` are the bucket when the rest matches `template`, or
/// `None` when it does not (or would leave no bucket).
fn bucket_labels_before(template: &str, labels: &[&str]) -> Option<usize> {
    let mut pattern: Vec<&str> = template.split('.').collect();
    let mut end = labels.len();
    if pattern.last() == Some(&"{dnsSuffix}") {
        pattern.pop();
        let suffix = PARTITION_DNS_SUFFIXES
            .iter()
            .filter(|suffix| ends_with_labels(&labels[..end], suffix))
            .map(|suffix| suffix.split('.').count())
            .max()?;
        end -= suffix;
    }
    for want in pattern.iter().rev() {
        end = end.checked_sub(1)?;
        let label = labels[end];
        let fits = match *want {
            "{region}" => label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            literal => label.eq_ignore_ascii_case(literal),
        };
        if !fits {
            return None;
        }
    }
    (end >= 1).then_some(end)
}

fn ends_with_labels(labels: &[&str], suffix: &str) -> bool {
    let suffix: Vec<&str> = suffix.split('.').collect();
    labels.len() >= suffix.len()
        && labels[labels.len() - suffix.len()..]
            .iter()
            .zip(&suffix)
            .all(|(label, want)| label.eq_ignore_ascii_case(want))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few templates in the shape the macro derives, enough to exercise the matcher. The
    /// complete S3 set, read off the real model, is covered by `restxml-e2e-tests`.
    const TEMPLATES: VirtualHostAddressing = VirtualHostAddressing::new(&[
        "s3.{region}.{dnsSuffix}",
        "s3.{dnsSuffix}",
        "s3.dualstack.{region}.{dnsSuffix}",
        "s3-accelerate.{dnsSuffix}",
    ]);

    fn split(host: &str) -> Option<(&str, &str)> {
        TEMPLATES.split(host).map(|v| (v.bucket, v.endpoint))
    }

    #[test]
    fn a_host_is_split_into_its_bucket_and_endpoint() {
        assert_eq!(
            split("b.s3.us-east-1.amazonaws.com"),
            Some(("b", "s3.us-east-1.amazonaws.com"))
        );
        assert_eq!(split("b.s3.amazonaws.com"), Some(("b", "s3.amazonaws.com")));
        assert_eq!(
            split("b.s3.dualstack.eu-west-1.amazonaws.com:8443"),
            Some(("b", "s3.dualstack.eu-west-1.amazonaws.com:8443"))
        );
        assert_eq!(
            split("b.s3-accelerate.amazonaws.com"),
            Some(("b", "s3-accelerate.amazonaws.com"))
        );
        assert_eq!(
            split("b.s3.cn-north-1.amazonaws.com.cn"),
            Some(("b", "s3.cn-north-1.amazonaws.com.cn"))
        );
        assert_eq!(
            split("B.S3.US-EAST-1.AMAZONAWS.COM."),
            Some(("B", "S3.US-EAST-1.AMAZONAWS.COM."))
        );
    }

    #[test]
    fn a_bucket_with_dots_stays_whole_and_the_rightmost_endpoint_ends_it() {
        assert_eq!(
            split("my.bucket.s3.us-east-1.amazonaws.com"),
            Some(("my.bucket", "s3.us-east-1.amazonaws.com"))
        );
        assert_eq!(
            split("a.s3.us-west-2.s3.us-east-1.amazonaws.com"),
            Some(("a.s3.us-west-2", "s3.us-east-1.amazonaws.com"))
        );
        assert_eq!(
            split("s3.s3.us-east-1.amazonaws.com"),
            Some(("s3", "s3.us-east-1.amazonaws.com"))
        );
    }

    #[test]
    fn a_path_style_endpoint_or_a_foreign_host_is_not_virtual_hosted() {
        for host in [
            "s3.us-east-1.amazonaws.com",
            "s3.amazonaws.com",
            "b.s3.us-east-1.example.com",
            "b.s3.us-east-1.amazonaws.com.evil.com",
            "127.0.0.1:4566",
            "localhost",
            "b.sqs.us-east-1.amazonaws.com",
            "",
        ] {
            assert_eq!(split(host), None, "{host}");
        }
    }

    #[test]
    fn a_host_that_is_not_plain_dns_is_never_split() {
        for host in [
            "victim/k.s3.amazonaws.com",
            "victim?x.s3.amazonaws.com",
            "victim#x.s3.amazonaws.com",
            "vic%74im.s3.amazonaws.com",
            "vic tim.s3.amazonaws.com",
            "b.s3.amazonaws.com:",
            "b.s3.amazonaws.com:80a",
            ".s3.amazonaws.com",
            "a..b.s3.amazonaws.com",
        ] {
            assert_eq!(split(host), None, "{host:?}");
        }
    }

    #[test]
    fn a_service_with_no_templates_matches_nothing() {
        assert_eq!(
            VirtualHostAddressing::new(&[]).split("b.s3.amazonaws.com"),
            None
        );
    }
}
