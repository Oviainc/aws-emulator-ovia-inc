// SPDX-License-Identifier: CC0-1.0

//! Amazon VPC Lattice's published default-quota catalog.

use super::*;

/// Amazon VPC Lattice's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `vpc-lattice` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code vpc-lattice`
/// reports, which is the "Quotas for Amazon VPC Lattice" page of its user guide), and the
/// `QuotaCode`s that catalog is keyed on.
///
/// The service's quotas are the planes of its service network — the networks of an
/// account, the services and VPCs associated with one, and the listeners, rules and
/// target groups a service routes through — so each row names the plane it counts. The
/// one row published as a RATE carries the one-second window AWS publishes it over.
pub(in crate::state) fn vpc_lattice_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Auth policies per resource", 1.0)
            .code("L-0DCA4434")
            .fixed()
            .describe("The maximum number of auth policies on one VPC Lattice resource."),
        quota("Certificate associations per service", 1.0)
            .code("L-3DEC3B9F")
            .fixed()
            .describe("The maximum number of certificates associated with one service."),
        quota("Header match conditions per rule", 5.0)
            .code("L-5FF8F9B9")
            .fixed()
            .describe("The maximum number of header match conditions in one rule."),
        quota("Listeners per service", 10.0)
            .code("L-6095700C")
            .describe("The maximum number of listeners on one service."),
        quota("Requests per second per service", 10_000.0)
            .code("L-620C821E")
            .period(1, "SECOND")
            .describe("The maximum number of requests per second that one service serves."),
        quota("Rules per listener", 100.0)
            .code("L-73D0F278")
            .describe("The maximum number of rules on one listener."),
        quota("Service network associations per service", 5.0)
            .code("L-75D4A19E")
            .describe("The maximum number of service networks one service is associated with."),
        quota("Service networks per account", 10.0)
            .code("L-87FCA9B2")
            .describe(
                "The maximum number of service networks in this account in the current \
                 Region.",
            ),
        quota("Service networks per VPC", 5.0)
            .code("L-89DEA27F")
            .describe("The maximum number of service networks one VPC is associated with."),
        quota("Services per account", 500.0)
            .code("L-9BC96FEF")
            .describe("The maximum number of services in this account in the current Region."),
        quota("Services per service network", 500.0)
            .code("L-9CAD07FB")
            .describe("The maximum number of services associated with one service network."),
        quota("Tags per resource", 50.0)
            .code("L-BB11C6B9")
            .fixed()
            .describe("The maximum number of tags on one VPC Lattice resource."),
        quota("Target groups per account", 1000.0)
            .code("L-CA6A1CC5")
            .describe(
                "The maximum number of target groups in this account in the current Region.",
            ),
        quota("Target groups per rule", 2.0)
            .code("L-CF78395E")
            .fixed()
            .describe("The maximum number of target groups one rule forwards to."),
        quota("Targets per target group", 1000.0)
            .code("L-D64E952E")
            .describe("The maximum number of targets in one target group."),
        quota("VPC associations per service network", 500.0)
            .code("L-D71303F3")
            .describe("The maximum number of VPCs associated with one service network."),
        quota("Weighted target groups per rule", 10.0)
            .code("L-EF6E2D62")
            .fixed()
            .describe("The maximum number of weighted target groups in one rule's action."),
    ]
}
