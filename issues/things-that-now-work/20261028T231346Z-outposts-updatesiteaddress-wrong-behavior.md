Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: baa5ed9f3060077952c4e32e7cba589107be720a

# An operating-address change is refused whenever an Outpost belongs to the site, even before any order is placed there

_Filed: 2026-10-28T23:13:46Z_
_Operation: UpdateSiteAddress | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `baa5ed9f3060077952c4e32e7cba589107be720a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents TWO windows in which a site's OPERATING address may be changed: "before you place an order at the site", OR "after all Outposts that belong to the site have been deactivated". The emulator implements only the second one, and implements it as "an Outpost record still names this site": `site_address_block` refuses an OPERATING change as soon as `outpost_at_site` finds any Outpost of the site, regardless of whether an order was ever placed at that site.

So the common flow CreateSite -> CreateOutpost -> UpdateSiteAddress(OPERATING_ADDRESS) is answered 409 `ConflictException`, with nothing written, even though no order exists anywhere in the store and the request therefore falls squarely inside the first documented window. The first window is never honoured at all: there is no code path on which an Outpost at the site plus zero orders results in a successful operating-address change.

The crate's own module doc quotes the sentence and then silently drops its first half, reading it as "additionally holds an OPERATING change while an Outpost still belongs to the site". Nothing new has to be stored to tell the two windows apart: `OrderRecord` already carries the `site_id` the order was placed at (that is how the sibling order-in-progress limit is evaluated), so "an order has been placed at this site" is already answerable from persisted state.

## Evidence

Smithy doc, `smithy_query(query="doc", shape="UpdateSiteAddress")` — the sentence states the permitted windows:

    "Updates the address of the specified site."
    "You can't update a site address if there is an order in progress. You must wait for the order to complete or cancel the order."
    "You can update the operating address before you place an order at the site, or after all Outposts that belong to the site have been deactivated."

The crate reproduces that same sentence in the handler's module doc and then narrows it — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_site_address.rs:17-20:

    //! outstanding, naming that order; "You can update the operating address before you
    //! place an order at the site, or after all Outposts that belong to the site have been
    //! deactivated" additionally holds an OPERATING change while an Outpost still belongs to
    //! the site, naming that Outpost.

What the code actually tests — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:4184-4193 (`site_address_block`), reached with no reference to any order:

        if kind != SiteAddressKind::Operating {
            return None;
        }
        let outpost_id = self
            .outpost_at_site(region, site_id)
            .and_then(|record| record.outpost.outpost_id.clone())?;
        Some(SiteAddressBlock {
            message: site_address_conflict(site_id, &outpost_id),
            resource: Some((RESOURCE_TYPE_OUTPOST, outpost_id)),
        })

and the refusal the handler answers with — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/update_site_address.rs:224-237:

        if let Some(block) = store.site_address_block(&region, &site_id, kind) {
            ...
            return Err(UpdateSiteAddressErrorModeled::ConflictException(ConflictException {
                message: Some(block.message),
                resource_id,
                resource_type,
            })
            .into());
        }

The fact needed for the first window is already persisted — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:1283-1293:

    /// The site the Outpost this order was placed on belongs to, as the Outpost
    /// named it when the order was submitted.
    ...
    pub site_id: Option<String>,

Test coverage: tests/all/site_address.rs:342-390 pins exactly the over-refusal. Its scenario creates a site and one Outpost and NO order (tests/all/site_address.rs:350-361) and then asserts the 409:

    /// Rule 3: while an Outpost belongs to the site, a change to the OPERATING address is
    /// refused 409 `ConflictException` naming that Outpost
    ...
            (reply.status, reply.error_code.as_str()),
            (409, "ConflictException"),

The test comment states no model basis for refusing a site at which no order has been placed, and the modelled sentence above permits that case.

## Steps to Reproduce

1. POST /sites with {"Name":"s","OperatingAddress":{"AddressLine1":"1 Main","City":"Seattle","StateOrRegion":"WA","PostalCode":"98101","CountryCode":"US"}} -> note SiteId
2. POST /outposts with {"Name":"o","SiteId":"<SiteId>"} -> an Outpost now belongs to the site; place NO order
3. GET /list-orders -> confirms the store holds no order at all, so the request below is inside the documented "before you place an order at the site" window
4. PUT /sites/<SiteId>/address with {"AddressType":"OPERATING_ADDRESS","Address":{...new...}} -> 409 ConflictException naming the Outpost, and GET /sites/<SiteId>/address?AddressType=OPERATING_ADDRESS still returns the old address
5. Expected per the modelled sentence: the change is accepted (no order has been placed at the site); the 409 is correct only once an order has been placed at the site and the site's Outposts are not all deactivated
