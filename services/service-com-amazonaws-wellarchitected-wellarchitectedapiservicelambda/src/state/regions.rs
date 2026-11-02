// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services Regions this API names, and the one a request that names none
//! is attributed to.
//!
//! A Region is a VALUE here — what a workload says it runs in, what an ARN carries, and
//! what a request is signed for. How the store is PARTITIONED by it, and what bounds that
//! partitioning, is [`super::account_regions`].
//!
//! `super` re-exports both items, so a caller names `crate::state::AWS_REGIONS` and
//! `crate::state::DEFAULT_REGION` as before.

/// The region an unsigned request is attributed to, when no SigV4 credential scope
/// names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Every Amazon Web Services Region id a workload may name in `AwsRegions`.
///
/// This model types `AwsRegion` as a plain string ("An Amazon Web Services Region, for
/// example, `us-west-2` or `ap-northeast-1`" — `smithy_query(query="shape",
/// shape="AwsRegion")`), so the value set is not in the model: it is the published
/// Region list from the AWS General Reference ("Regions, Availability Zones, and Local
/// Zones"), which other AWS Smithy models enumerate verbatim as an `AwsRegion` enum.
/// Sorted, so a reader can see at a glance what is and is not here.
pub const AWS_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-east-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-6",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "cn-north-1",
    "cn-northwest-1",
    "eu-central-1",
    "eu-central-2",
    "eu-isoe-west-1",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eusc-de-east-1",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-gov-east-1",
    "us-gov-west-1",
    "us-iso-east-1",
    "us-iso-west-1",
    "us-isob-east-1",
    "us-isob-west-1",
    "us-isof-east-1",
    "us-isof-south-1",
    "us-west-1",
    "us-west-2",
];
