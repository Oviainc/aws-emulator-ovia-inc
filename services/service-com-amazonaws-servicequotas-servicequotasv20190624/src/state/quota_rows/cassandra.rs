// SPDX-License-Identifier: CC0-1.0

//! Amazon Keyspaces (for Apache Cassandra)'s published default-quota catalog.

use super::*;

/// Amazon Keyspaces (for Apache Cassandra)'s published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cassandra` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code cassandra` reports, which
/// is the "Quotas for Amazon Keyspaces" page of the Amazon Keyspaces Developer Guide), and
/// the recorded default-quota listing of that code for which `QuotaCode` names which
/// quota, for the `Max …` wording the service publishes its rows under and for each row's
/// unit and adjustability. Every row carries the `L-` code AWS publishes the quota under,
/// so a client that read a code out of AWS's catalog resolves the same quota here.
pub(in crate::state) fn cassandra_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Max clustering key size", 850.0)
            .code("L-0659E12E")
            .unit("Bytes")
            .fixed()
            .describe("The maximum combined size, in bytes, of one row's clustering columns."),
        quota("Account-level read throughput quota", 80_000.0)
            .code("L-089904FB")
            .describe(
                "The maximum number of read capacity units that can be provisioned across all \
                 tables in this account in the current Region.",
            ),
        quota("Table-level read throughput quota", 40_000.0)
            .code("L-17766544")
            .describe(
                "The maximum number of read capacity units that can be provisioned for a \
                 single table.",
            ),
        quota("Account-level write throughput quota", 80_000.0)
            .code("L-29E90199")
            .fixed()
            .describe(
                "The maximum number of write capacity units that can be provisioned across all \
                 tables in this account in the current Region.",
            ),
        quota("Table-level write throughput quota", 40_000.0)
            .code("L-2C5B14BD")
            .describe(
                "The maximum number of write capacity units that can be provisioned for a \
                 single table.",
            ),
        quota("Max number of keyspaces per AWS Region", 256.0)
            .code("L-2FC1B9A1")
            .describe(
                "The maximum number of keyspaces in this account in the current Region.",
            ),
        quota("Max number of tables per AWS Region", 256.0)
            .code("L-324B5396")
            .describe("The maximum number of tables in this account in the current Region."),
        quota("Max row size", 1.0)
            .code("L-3D8ED127")
            .unit("Megabytes")
            .fixed()
            .describe("The maximum size, in MB, of one row."),
        quota("Max partition key size", 2048.0)
            .code("L-4C49F3DB")
            .unit("Bytes")
            .fixed()
            .describe("The maximum combined size, in bytes, of one row's partition key columns."),
        quota("Max static data per logical partition", 1.0)
            .code("L-50DCCF0B")
            .unit("Megabytes")
            .fixed()
            .describe("The maximum size, in MB, of the static data of one logical partition."),
        quota("Max number of columns per table", 225.0)
            .code("L-5823D982")
            .fixed()
            .describe("The maximum number of columns in one table."),
        quota("Max number of concurrent table operations", 10.0)
            .code("L-6632AD49")
            .describe(
                "The maximum number of table creations, deletions and updates in progress at \
                 once in this account in the current Region.",
            ),
        quota("Max number of concurrent restore operations", 4.0)
            .code("L-66AD5703")
            .describe(
                "The maximum number of table restores in progress at once in this account in \
                 the current Region.",
            ),
        quota("Max number of tags per resource", 50.0)
            .code("L-66D0104D")
            .fixed()
            .describe("The maximum number of tags on one keyspace or table."),
        quota("Max TTL", 630_720_000.0)
            .code("L-677FFD22")
            .unit("Seconds")
            .fixed()
            .describe("The maximum Time to Live, in seconds, that can be set on a row."),
        quota("Max number of UDTs per AWS Region", 5000.0)
            .code("L-7A88D508")
            .describe(
                "The maximum number of user-defined types (UDTs) in this account in the current \
                 Region.",
            ),
        quota("Max number of UDTs per table", 10.0)
            .code("L-7F2EAE05")
            .fixed()
            .describe("The maximum number of user-defined types (UDTs) used by one table."),
        quota("Max number of fields per UDT", 100.0)
            .code("L-964C49BD")
            .fixed()
            .describe("The maximum number of fields in one user-defined type (UDT)."),
        quota("Max nested depth per UDT", 5.0)
            .code("L-96FEFC6D")
            .fixed()
            .describe(
                "The maximum depth a user-defined type (UDT) can be nested inside other UDTs \
                 to.",
            ),
        quota("Max length of a UDT name", 48.0)
            .code("L-BF48748A")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of a user-defined type (UDT) name."),
        quota("Max number of multi-Region replicas per table", 6.0)
            .code("L-C63C913D")
            .fixed()
            .describe("The maximum number of Regions one multi-Region table is replicated to."),
        quota("Max size of a single column value", 1.0)
            .code("L-D5E49767")
            .unit("Megabytes")
            .fixed()
            .describe("The maximum size, in MB, of the value of one column."),
        quota("Max number of CQL statements per batch", 30.0)
            .code("L-E08A04C1")
            .fixed()
            .describe("The maximum number of statements in one unlogged batch."),
        quota("Max number of partition keys per IN clause", 100.0)
            .code("L-E7DB0CFF")
            .fixed()
            .describe("The maximum number of partition keys one IN clause can select."),
        quota("Max number of simultaneous CQL connections", 3000.0)
            .code("L-F41E662B")
            .describe(
                "The maximum number of simultaneous CQL client connections in this account in \
                 the current Region.",
            ),
        quota("Max amount of direct child UDTs per UDT", 10.0)
            .code("L-F90953AC")
            .describe(
                "The maximum number of user-defined types (UDTs) one UDT can refer to directly.",
            ),
        quota("Max number of columns per partition key", 10.0)
            .code("L-FDBA801F")
            .fixed()
            .describe("The maximum number of columns in one table's partition key."),
    ]
}
