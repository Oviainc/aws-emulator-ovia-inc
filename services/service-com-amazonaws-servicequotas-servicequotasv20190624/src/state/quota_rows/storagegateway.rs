// SPDX-License-Identifier: CC0-1.0

//! AWS Storage Gateway's published default-quota catalog.

use super::*;

/// AWS Storage Gateway's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `storagegateway` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// storagegateway` reports, which is the "AWS Storage Gateway quotas" pages of its user
/// guide), and the `QuotaCode`s that catalog is keyed on.
///
/// Storage Gateway publishes its quotas PER GATEWAY TYPE — the volumes a volume gateway
/// serves, the shares a file gateway serves and the virtual tapes a tape gateway serves —
/// which is why most rows are named for the plane of one gateway rather than for the
/// account. A row that measures a capacity carries the unit AWS publishes it in rather
/// than being a bare count.
pub(in crate::state) fn storagegateway_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Gateways per Region", 100.0)
            .code("L-14F83003")
            .describe(
                "The maximum number of gateways in this account in the current Region.",
            ),
        quota("Cached volumes per gateway", 32.0)
            .code("L-2232E8E3")
            .fixed()
            .describe("The maximum number of cached volumes per volume gateway."),
        quota("Stored volumes per gateway", 32.0)
            .code("L-2E88EE16")
            .fixed()
            .describe("The maximum number of stored volumes per volume gateway."),
        quota("Maximum size of a cached volume", 32.0)
            .code("L-2EC26EAB")
            .unit("TiB")
            .fixed()
            .describe("The maximum size, in TiB, of one cached volume."),
        quota("Maximum size of a stored volume", 16.0)
            .code("L-311F8856")
            .unit("TiB")
            .fixed()
            .describe("The maximum size, in TiB, of one stored volume."),
        quota("Total cached volume storage per gateway", 1024.0)
            .code("L-4951D254")
            .unit("TiB")
            .fixed()
            .describe(
                "The maximum total size, in TiB, of the cached volumes of one volume \
                 gateway.",
            ),
        quota("Total stored volume storage per gateway", 512.0)
            .code("L-5308FBCA")
            .unit("TiB")
            .fixed()
            .describe(
                "The maximum total size, in TiB, of the stored volumes of one volume \
                 gateway.",
            ),
        quota("File shares per file gateway", 50.0)
            .code("L-58DCC961")
            .fixed()
            .describe("The maximum number of file shares per file gateway."),
        quota("Maximum size of a file", 5.0)
            .code("L-59D49F15")
            .unit("TiB")
            .fixed()
            .describe("The maximum size, in TiB, of one file written through a file share."),
        quota("Virtual tapes per gateway", 1500.0)
            .code("L-6D83F84B")
            .fixed()
            .describe("The maximum number of virtual tapes per tape gateway."),
        quota("Maximum size of a virtual tape", 5.0)
            .code("L-6F75AC83")
            .unit("TiB")
            .fixed()
            .describe("The maximum size, in TiB, of one virtual tape."),
        quota("Minimum size of a virtual tape", 100.0)
            .code("L-761F1680")
            .unit("Gigabytes")
            .fixed()
            .describe("The minimum size, in GB, of one virtual tape."),
        quota("Total virtual tape storage per gateway", 1024.0)
            .code("L-7B01AFFB")
            .unit("TiB")
            .fixed()
            .describe(
                "The maximum total size, in TiB, of the virtual tapes of one tape gateway.",
            ),
        quota("Virtual tapes in the tape archive", 10_000.0)
            .code("L-803470D8")
            .describe("The maximum number of virtual tapes in the tape archive."),
        quota("Snapshot schedules per volume", 1.0)
            .code("L-81A6E497")
            .fixed()
            .describe("The maximum number of snapshot schedules per volume."),
        quota("Cache disks per gateway", 20.0)
            .code("L-87640A7E")
            .fixed()
            .describe("The maximum number of local disks used as cache by one gateway."),
        quota("Upload buffer disks per gateway", 20.0)
            .code("L-96D17DE9")
            .fixed()
            .describe(
                "The maximum number of local disks used as upload buffer by one gateway.",
            ),
        quota("Cache storage per gateway", 64.0)
            .code("L-98F44425")
            .unit("TiB")
            .fixed()
            .describe("The maximum cache storage, in TiB, of one gateway."),
        quota("Upload buffer storage per gateway", 2.0)
            .code("L-99E991AF")
            .unit("TiB")
            .fixed()
            .describe("The maximum upload buffer storage, in TiB, of one gateway."),
        quota("Custom tape pools per account", 100.0)
            .code("L-BD85E1A6")
            .describe(
                "The maximum number of custom tape pools in this account in the current \
                 Region.",
            ),
        quota("Automatic tape creation policies per gateway", 1.0)
            .code("L-C86341FE")
            .fixed()
            .describe(
                "The maximum number of automatic tape creation policies per tape gateway.",
            ),
        quota("Bandwidth rate limit schedule intervals per gateway", 20.0)
            .code("L-CEFD39A0")
            .fixed()
            .describe(
                "The maximum number of intervals in the bandwidth rate limit schedule of \
                 one gateway.",
            ),
        quota("Clients per NFS file share", 100.0)
            .code("L-E0AEEBAC")
            .fixed()
            .describe("The maximum number of clients allowed by one NFS file share."),
        quota("Tags per resource", 50.0)
            .code("L-F3EDDF8C")
            .fixed()
            .describe("The maximum number of tags on one Storage Gateway resource."),
        quota("Volumes per snapshot schedule", 1.0)
            .code("L-F5915598")
            .fixed()
            .describe("The maximum number of volumes covered by one snapshot schedule."),
        quota("Audit logs per file share", 1.0)
            .code("L-FF1BE522")
            .fixed()
            .describe("The maximum number of audit log destinations per file share."),
    ]
}
