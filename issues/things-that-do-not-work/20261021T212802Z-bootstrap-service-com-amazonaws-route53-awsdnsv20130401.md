Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Route 53 emulator

_Filed: 2026-10-21T21:28:02Z_
_Operation: service-com-amazonaws-route53-awsdnsv20130401 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and the crate holds no state. Implement a FIRST, SMALL, stateful slice: the hosted-zone + record-set happy path, which is what essentially every Route 53 client script does.

### Happy path to implement (in order)

1. **CreateHostedZone** — `POST /2013-04-01/hostedzone -> 201`. Input: `Name` (required), `CallerReference` (required), optional `HostedZoneConfig` (Comment, PrivateZone), `VPC`, `DelegationSetId`. Must mint a zone id (e.g. `Z` + 14 uppercase alphanumerics), normalize `Name` to a trailing dot, store the zone, and return `HostedZone{Id: "/hostedzone/<id>", Name, CallerReference, Config, ResourceRecordSetCount}`, a `DelegationSet{NameServers:[4 ns-*.awsdns-*.* names]}`, a `ChangeInfo{Id, Status: PENDING|INSYNC, SubmittedAt}` and the `Location` header `.../2013-04-01/hostedzone/<id>`. A repeated `CallerReference` must fail `HostedZoneAlreadyExists` (HTTP 409). For a public zone, auto-create the default SOA and NS records in the zone's record set (the model's own docs state Route 53 creates them), so `ResourceRecordSetCount` starts at 2.
2. **ChangeResourceRecordSets** — `POST /2013-04-01/hostedzone/{HostedZoneId}/rrset -> 200`. Input: `HostedZoneId` (httpLabel), `ChangeBatch{Comment, Changes:[{Action: CREATE|DELETE|UPSERT, ResourceRecordSet{Name, Type, TTL, ResourceRecords, SetIdentifier, ...}}]}`. Must apply every change atomically against the stored zone keyed by (Name, Type, SetIdentifier): CREATE on an existing key fails, DELETE of a non-existent record fails, UPSERT replaces or inserts. Unknown zone id => `NoSuchHostedZone` (404). Return a NEW `ChangeInfo{Id: "/change/<id>", Status, SubmittedAt}` that is persisted so GetChange can find it. Accept both bare and `/hostedzone/`-prefixed ids.
3. **ListResourceRecordSets** — `GET /2013-04-01/hostedzone/{HostedZoneId}/rrset -> 200`. Must return the records actually stored by step 2 (plus the auto-created SOA/NS), sorted in Route 53 order (DNS-name order, then type, then SetIdentifier), honoring `StartRecordName`/`StartRecordType`/`StartRecordIdentifier` and `MaxItems`, with `IsTruncated` and `NextRecordName`/`NextRecordType`/`NextRecordIdentifier` computed from the remainder. Unknown zone => `NoSuchHostedZone` (404).
4. **GetChange** — `GET /2013-04-01/change/{Id} -> 200`. Must look up the change id minted by CreateHostedZone/ChangeResourceRecordSets and return its persisted `ChangeInfo`; an unknown id must be `NoSuchChange` (404), never a fabricated INSYNC response.
5. **GetHostedZone** / **ListHostedZones** — `GET /2013-04-01/hostedzone/{Id}` and `GET /2013-04-01/hostedzone`. Get must return the stored `HostedZone` (with a live `ResourceRecordSetCount`), its `DelegationSet` and any associated `VPCs`, and `NoSuchHostedZone` (404) after deletion. List must page over the stored zones using `Marker`/`MaxItems` and set `Marker`, `IsTruncated`, `NextMarker`, `MaxItems` (model marks this operation `@paginated(inputToken=Marker, outputToken=NextMarker, items=HostedZones)`).
6. **DeleteHostedZone** — `DELETE /2013-04-01/hostedzone/{Id} -> 200`. Must refuse with `HostedZoneNotEmpty` (400) while the zone still holds records other than the default SOA/NS, remove the zone otherwise, and return a persisted `ChangeInfo`. After it, GetHostedZone must 404 and ListHostedZones must omit the zone.

Every one of these must compute its response from persisted state held in the crate's shared engine state (`crate::state::service_state`). No canned bodies, no echoing the input back, no minting an id that is never stored — a create followed by a get/list must return exactly what was created, and a delete must be observable by later reads.

## State Model

- `zones: HashMap<String /* zone id, e.g. Z1D633PJN98FT9 */, HostedZone>` where `HostedZone { id, name /* normalized with trailing dot */, caller_reference, comment: Option<String>, private_zone: bool, delegation_set_id: Option<String>, name_servers: Vec<String>, vpcs: Vec<Vpc { vpc_id, vpc_region }>, records: BTreeMap<RecordKey, ResourceRecordSet>, created_at }`
- `RecordKey { name_sort_key: String /* DNS-order normalized name */, rr_type: String, set_identifier: Option<String> }` so ordering and (Name,Type,SetIdentifier) uniqueness both fall out of the map.
- `ResourceRecordSet { name, rr_type, ttl: Option<i64>, resource_records: Vec<String>, set_identifier: Option<String>, weight/region/geo_location/failover: Option<..>, alias_target: Option<AliasTarget>, health_check_id: Option<String> }`
- `caller_references: HashSet<String>` (or an index into `zones`) for the `HostedZoneAlreadyExists` idempotency check.
- `changes: HashMap<String /* change id */, ChangeInfo { id, status, submitted_at, comment }>` populated by CreateHostedZone, ChangeResourceRecordSets and DeleteHostedZone, read by GetChange.
- A monotonic counter / RNG seed for zone, change and delegation-set id minting, kept in the same state struct so ids are unique across calls.

Everything else (health checks, traffic policies, DNSSEC/key signing keys, CIDR collections, query logging configs, reusable delegation sets, tags, TestDNSAnswer, geo locations, limits/counts) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-route53-awsdnsv20130401/OPERATIONS.md` defines the routes, input/output members and errors cited above: ChangeResourceRecordSets (line 47), CreateHostedZone (line 75), DeleteHostedZone (line 152), GetChange (line 229), GetHostedZone (line 285), ListHostedZones (line 387, with the `@paginated` trait), ListResourceRecordSets (line 417). `src/ops/create_hosted_zone.rs` lines 4-12 carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 254 returns `Scaffolded::not_implemented("CreateHostedZone")`; all 72 files in `src/ops/` are the same generated stub, so the service holds zero state and every call answers HTTP 501. The default SOA + 4 NS records on public zone creation and the PENDING->INSYNC change lifecycle are documented in the model's own CreateHostedZone description (quoted in that file's header, lines 60-73).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Route 53 client at it.
2. `aws route53 create-hosted-zone --name example.com --caller-reference ref-1` -> expect 201 with a minted `/hostedzone/<id>`, a 4-name-server DelegationSet, a ChangeInfo and a Location header (today: HTTP 501 NotImplemented).
3. Repeat the same call with `--caller-reference ref-1` -> expect HTTP 409 HostedZoneAlreadyExists.
4. `aws route53 get-change --id <ChangeInfo.Id>` -> expect the persisted ChangeInfo; `get-change --id /change/BOGUS` -> expect 404 NoSuchChange.
5. `aws route53 change-resource-record-sets --hosted-zone-id <id> --change-batch '{"Changes":[{"Action":"CREATE","ResourceRecordSet":{"Name":"www.example.com","Type":"A","TTL":300,"ResourceRecords":[{"Value":"192.0.2.1"}]}}]}'` -> expect 200 with a new ChangeInfo.
6. `aws route53 list-resource-record-sets --hosted-zone-id <id>` -> expect the SOA, NS and the new www.example.com A record with TTL 300 and value 192.0.2.1, in DNS sort order.
7. Repeat the identical CREATE change -> expect HTTP 400 InvalidChangeBatch; then UPSERT the same name/type to 192.0.2.2 and re-list -> expect exactly one A record with the new value.
8. `aws route53 get-hosted-zone --id <id>` -> expect the stored name/comment plus ResourceRecordSetCount reflecting the records; `aws route53 list-hosted-zones --max-items 1` -> expect IsTruncated/NextMarker paging over the created zones.
9. `aws route53 delete-hosted-zone --id <id>` while the A record exists -> expect HTTP 400 HostedZoneNotEmpty; DELETE the A record, then delete the zone -> expect 200 with a ChangeInfo.
10. `aws route53 get-hosted-zone --id <id>` -> expect HTTP 404 NoSuchHostedZone, and the zone absent from list-hosted-zones.
