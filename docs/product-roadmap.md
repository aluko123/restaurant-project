# Parline product roadmap

**North star:** Import quickly. Count accurately. Purchase confidently.

Parline is building toward an affordable, inventory-first alternative to full restaurant back-office suites (for example Restaurant365), without becoming accounting, payroll, or POS software.

## Vision

Help independent restaurants protect margin by turning messy operational data into a short list of source-backed daily actions.

Keep the tools restaurants already use:

- Keep the POS.
- Snap or forward invoices.
- Count what matters.
- Export clean summaries for the bookkeeper later.

Do not replace:

- POS systems
- Payment processing
- Full accounting / general ledger
- Payroll
- Scheduling

## Current focus

| Decision | Choice |
| --- | --- |
| Initial customer | Single-location restaurants |
| Product depth | Counts + purchasing |
| POS connector | Discover from first pilots before choosing one |
| Positioning | Inventory-first Restaurant365 alternative at lower cost and setup effort |
| First-use promise | Connect what you have, upload what you can, and get a useful action in the first shift |

### What “counts + purchasing” means

Build first:

1. Fast migration / onboarding
2. Excellent physical inventory counts
3. Purchasing guided by counts, pars, and invoices
4. A daily brief of inventory and purchasing actions

Defer full perpetual inventory until the count and purchase loops are excellent.

## Inventory accuracy boundary

Until perpetual inventory exists, Parline should say **last counted quantity**, not **current on hand**.

Suggested orders should come from:

- a fresh count
- par levels
- open orders / receipts
- recent purchase prices

Do not pretend sales automatically depleted stock until recipe usage and inventory movements are real.

## Recommended build order

### Phase 1 — Cohesive migration onboarding

Goal: a new restaurant becomes useful in one shift.

1. Create account (Google / magic link).
2. Capture restaurant basics (name, city, service style, timezone).
3. Ask which POS and accounting tools they use (no forced integration).
4. Present one guided setup for the records they already have:
   - inventory CSV
   - menu photo / PDF / CSV
   - recent supplier invoices
   - POS sales connection or sales CSV
5. Preview every import, identify duplicates and unmatched units, and preserve its source.
6. Batch-upload recent invoices to seed suppliers, products, and prices.
7. Suggest the top 20 items to count first based on cost, volume, stockouts, and operational importance—not spreadsheet order.
8. Guide the user into their first physical count and count-backed order guide.

Build this as progressive setup, not a blocking wizard. A restaurant should be able to skip a source it does not have and return later.

Success looks like:

- Setup under 10 minutes of active owner time
- Most inventory rows import without hand-keying
- At least one useful source is connected or imported during setup
- First count starts the same day
- The first source-backed action appears during the first shift

### Phase 2 — Excellent inventory counts

Goal: counting is fast enough that managers actually do it.

- Count by storage area and shelf order
- Mobile-first counting with saved drafts
- Previous-count comparison and variance review
- Explicit confirmation for skipped items
- Count history and reusable count templates
- Optional low / okay / full mode for early pilots

### Phase 3 — Purchasing

Goal: after a count, the next order is obvious.

- Canonical suppliers and supplier products
- Par levels and preferred suppliers
- Editable order guide generated after a count
- Mark ordered, receive delivery, attach invoice
- Flag quantity discrepancies and supplier price changes
- Reuse supplier-product mappings over time

### Phase 4 — Operational Today brief

Goal: every morning answers “what should we do next?”

Prioritize:

- Items below par
- Counts due
- Orders awaiting receipt
- Invoice reviews / discrepancies
- Meaningful supplier price changes
- Recent waste or stockouts that change purchasing

Every action should include:

1. What changed
2. Why it matters
3. Recommended next step
4. Confidence / evidence when available

### Phase 5 — Source connections and durable sync

Goal: stop asking restaurants to repeatedly upload data their existing tools can provide.

1. Interview the first 5–10 pilot restaurants.
2. Choose the POS used most often.
3. Keep CSV as the universal fallback.
4. Start with read-only location, menu, and sales sync.
5. Preserve external IDs, sync history, and reconnect states.
6. Add additional connectors only when pilot demand justifies them.

If pilot data is unknown:

- Prefer the POS with the strongest pilot demand.
- Square is often the fastest direct connector to build.
- Toast is strategically important but partner access can take longer.
- Clover is a common independent option.
- Legacy on-prem POS can wait for an aggregator when demand appears.

### Phase 6 — Demand forecasting and external context

Goal: improve purchasing and prep recommendations without hiding uncertainty.

Build in this order:

1. Establish a restaurant-specific baseline from historical item sales.
2. Add day-of-week, seasonality, holidays, and recent trend.
3. Add weather when it measurably improves the baseline.
4. Add local events, reservations, traffic, and tourism signals where reliable data exists.
5. Show the factors that changed each forecast and a confidence level.

Do not let an external signal override weak or missing restaurant data without saying so.

### Phase 7 — Supplier alternatives and regional benchmarks

Goal: help restaurants identify credible purchasing alternatives.

Start with the restaurant's own evidence:

- Compare normalized unit prices across its known suppliers.
- Include pack size, delivery days, minimums, fees, substitutions, and availability.
- Suggest a known alternative only when products and units are comparable.
- Add participating supplier catalogs and anonymized regional benchmarks later.

Do not describe a supplier as “better” from price alone, and do not imply access to live contract pricing without a supplier feed.

## Restaurant data sources

Parline should support a connect-or-import path for each evidence stream. “All sources” is an adapter strategy, not a requirement that every integration ship before pilots.

| Evidence | Common sources | Universal fallback |
| --- | --- | --- |
| Sales and customer orders | Toast, Square, Clover, Lightspeed, SpotOn, TouchBistro, Revel | Sales CSV |
| Delivery orders | DoorDash, Uber Eats, Grubhub, ChowNow | Platform export |
| Inventory | Spreadsheets, paper counts, inventory systems | Inventory CSV + physical count |
| Supplier purchases | Broadline and local suppliers, purchasing systems | Invoice PDF/photo/CSV/email |
| Menu and prices | POS, website, menu systems | Menu photo/PDF/CSV |
| Recipes and portions | Recipe systems, spreadsheets, chef knowledge | Top-item manual setup |
| Accounting | QuickBooks, Xero, bookkeeper files | Purchase/export summary |
| Labor and reservations | 7shifts, Homebase, OpenTable, Resy, Tock | Later connector/export |
| External demand | Weather, holidays, events, reservations, traffic | Location-based data services |

Keep customer orders from the POS distinct from purchase orders sent to suppliers.

## Data migration standard

Call this **connecting and importing**, not full system replacement. The POS remains the source of truth for sales.

| Path | Use for |
| --- | --- |
| POS connection | Location, menu, item sales, historical sales |
| CSV / XLSX | Inventory lists, unsupported POS sales, old spreadsheets |
| Menu photo / PDF | Initial menu setup |
| Batch invoice upload / email | Supplier and price history |
| Concierge import | Messy pilot data that cannot be self-served |

Recommended initial history:

- Sales: 90 days immediately; backfill up to 12 months asynchronously
- Invoices: 60–90 days, or at least two invoices per major supplier
- Inventory: catalog + pars, preferably top 20 first
- Recipes: top-selling menu items only; never block onboarding on complete recipes

Every import needs:

- Preview before apply
- Duplicate detection
- Mapping for unmatched items
- External source IDs
- Import history and error report
- Safe retries without duplicated records
- Clear source ownership when manual edits conflict with resyncs

## Current product foundations

Already useful:

- Owner onboarding (name, city, service style)
- WorkOS auth and team invitations
- Sales manual entry and Sales CSV v1
- Menu document extraction
- Invoice upload, extraction, review, and purchase matching
- Inventory catalog, draft counts, and completion
- Inventory CSV import with preview, duplicate detection, safe retry, and first-count handoff
- Count-backed editable order guides, mark ordered, and partial receiving
- Progressive migration checklist with durable source counts, optional POS/accounting context, and an explicit finish-for-now action
- Waste / stockout logging
- Today actions and weekly brief preview

Still needed for this roadmap:

- Tool-specific connection recommendations and an editable setup summary
- Inventory XLSX import and clearer unmatched-unit review
- Batch / email invoice ingestion
- Supplier master (not free-text only)
- Storage areas and count templates
- Invoice attachment and discrepancy review during order receiving
- Durable sales import / sync provenance
- External IDs and mapping tables
- POS connector installation model
- Bookkeeper export basics

## Deferred

Do not build these before the inventory loop is strong:

- Multi-location transfers and rollups
- Full accounting / general ledger
- Bill pay
- Payroll and scheduling
- Recipe-driven theoretical usage as a blocker
- Automatic order placement to suppliers
- Real-time perpetual inventory as the first release
- Enterprise multi-unit controls

## Competitive context

Study these products for specific lessons, not feature parity:

| Product | Study for |
| --- | --- |
| MarginEdge | Invoice-first value and daily cost visibility |
| MarketMan | Purchasing depth and setup burden |
| Restaurant365 | Mature multi-module model and heavy implementation cost |
| xtraCHEF | Toast-native invoice and costing onboarding |
| Craftable | Mobile ops and vendor connectivity |
| WISK | Kitchen / bar counting UX |
| ClearCOGS | Daily operational recommendations from POS data |
| Tenzo | Role-specific nudges and data aggregation |
| Nory | Forecast-to-action workflows |
| meez | Recipe migration and culinary UX |

Parline’s wedge:

> Every morning, three source-backed actions across purchases, counts, prices, and losses — without replacing the POS or bookkeeper.

## Success measures

Near-term:

- Time to first useful brief
- Percent of inventory imported without manual re-entry
- Count completion rate
- Invoice-to-purchase match rate
- Owner retention after week 1 and week 4

Product quality:

- Actions are understandable in under 5 seconds
- Mobile workflows work in a kitchen
- Evidence is attached to every recommendation
- Language stays plain and restaurant-specific

## Open decisions

- Which POS appears most among the first 5–10 pilots?
- Which POS and accounting tool choices should onboarding capture first?
- Whether XLSX support is urgent after observing pilot CSV files
- Whether invoice email forwarding ships with batch upload or immediately after
- When low / okay / full counting is enough versus exact quantities
- When bookkeeper CSV/PDF export becomes urgent
- Which external signals measurably improve forecasts for the first pilot segment?

## Working principles

1. **Action over dashboard**
2. **Top 20 before track everything**
3. **Keep existing tools**
4. **Progressive precision**
5. **Human control** — approve, edit, dismiss; never silently act
6. **Evidence attached**
7. **Mobile first**
8. **Single location first**, multi-location later
