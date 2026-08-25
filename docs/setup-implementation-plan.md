# Parline setup implementation plan

## Outcome

Parline should launch a restaurant into a usable inventory and purchasing workflow without pretending that connecting a POS is the same as completing setup.

Every restaurant uses the same setup milestones, with two ways through them:

1. **Launch with Parline (recommended):** Parline prepares imports, mappings, and the first-count list; the restaurant authorizes connections and confirms restaurant-specific facts.
2. **Set it up myself:** the restaurant follows the same milestones through guided connectors, imports, and reviews.

Pricing, subscription gating, trials, and billing are deliberately outside this plan until the commercial model is validated.

## Product rules

- Organize setup around data streams, not vendor logos.
- Square is the first real connector and reference adapter, not a claim that every listed POS has direct connectivity.
- Always provide a connector, import, manual, assisted, or deferred path for each stream.
- Persist customer intent, ownership, and deferral; derive operational readiness from imports, connections, reviews, and counts.
- Leaving setup is not the same as completing every source.
- Setup activation requires a useful operational outcome, not merely one uploaded record.
- The restaurant must authorize external accounts and verify physical quantities and ambiguous mappings.
- Parline may prepare data but must not invent stock levels, recipe quantities, or uncertain unit conversions.

## Shared milestones

1. Restaurant basics and timezone
2. Setup approach selected
3. Menu and sales path selected
4. Inventory catalog available
5. Supplier purchases normalized
6. First physical count ready or started
7. Important menu items queued for ingredient setup
8. First-shift handoff and activation

The initial activation target is a usable inventory catalog plus a first count ready to perform. Menu, sales, invoices, and recipes improve the result without all becoming activation blockers.

## Source contract

Each setup stream will eventually have:

- `stream`: `menu`, `sales`, `inventory`, `purchases`, or `bookkeeping_export`
- `method`: `connector`, `import`, `manual`, `assisted`, or `deferred`
- `provider`: optional provider key such as `square`, `toast`, or `quickbooks_online`
- `owner`: `restaurant` or `parline`
- `status`: customer-facing workflow state such as `waiting_on_restaurant`, `waiting_on_parline`, `processing`, `needs_review`, `ready`, `error`, or `deferred`
- a concise next action

Do not duplicate facts such as sync success, import review status, record counts, or first-count completion in this contract. Derive those from their authoritative domain records.

## Connector contract

Before adding another direct POS connector, generalize the connection boundary around:

- authorization and reauthorization
- disconnect
- full and incremental sync
- connection health
- per-stream sync results
- external identity and provenance
- recoverable versus terminal errors
- declared capabilities such as locations, menu, item sales, daily totals, and history

Square remains the first implementation while unsupported POS systems use guided exports or assisted migration.

Bookkeeping systems are initially export destinations rather than equivalent POS sources. Do not ask for an accounting product unless setup can offer a concrete action or assisted handoff.

## Ordered delivery

### Phase 1 — Setup entry foundation

- [x] Store this plan in the repository.
- [x] Collect restaurant timezone during creation.
- [x] Remove duplicated POS/accounting questions from restaurant creation.
- [x] Persist `assisted` versus `self_service` setup approach and the active assistance-request time.
- [x] Present Launch with Parline as the recommended path and self-service as the alternative.
- [x] Preserve the existing setup route and records when switching approaches.
- [x] Only show Square controls when Square is selected or already connected.
- [x] Redirect to setup immediately after creation without blocking later Today visits.
- [x] Show a resumable setup prompt on Today while setup remains unfinished.
- [x] Enforce restaurant/connection pairing for sync jobs in the database and worker queries.
- [x] Require an independent token-encryption key and use cryptographic nonce generation.
- [x] Keep production Square OAuth scopes read-only and prevent disconnect races from reviving a connection.

### Phase 2 — Resumable source plans

- [x] Persist stream, method, provider, owner, and deferral.
- [x] Derive readiness and next actions from domain records.
- [x] Separate setup exit, activation, and source readiness.
- [x] Represent restaurant versus Parline ownership.
- [x] Keep navigation available while setup is unfinished.

### Phase 3 — Connector boundary and Square lifecycle

- [x] Generalize provider-neutral connection APIs and frontend state.
- [x] Preserve Square authorization and sync behavior as the first adapter.
- [x] Report menu and sales sync outcomes separately.
- [x] Treat OAuth completion as importing until initial sync succeeds.
- [x] Remove server configuration messaging from unrelated restaurants.

### Phase 4 — Invoice normalization

- [x] Fold supplier-product resolution into invoice review.
- [x] Auto-apply exact previously confirmed mappings with an editable summary.
- [x] Ask only about new, ambiguous, conflicting, or changed-unit lines.
- [x] Learn intentionally ignored supplier lines.
- [x] Record the purchase receipt as invoice review completes.
- [x] Replace recurring Connect purchases actions with exception review and View receipt.
- [x] Store price changes as reviewable findings and baseline historical setup imports.

### Phase 5 — First-count handoff

- [x] Prepare a focused first-count list from imported inventory.
- [x] Surface unresolved critical units before counting.
- [x] Provide assisted and self-service handoff summaries from the same evidence.
- [x] Activate on a usable first-count outcome rather than source count totals.

### Phase 6 — Post-activation source health and Today

- [x] Convert Setup into compact source health after activation.
- [x] Show provenance, last success, backlog, and recoverable errors.
- [x] Move optional source coaching out of Today.
- [x] Let domain tasks disappear when completed.
- [x] Let durable findings remain reviewable; do not show a snooze control without evidence.

### Phase 7 — Important menu-item ingredients

- [x] Offer Add ingredients now or Later after manual menu-item creation.
- [x] Use a resumable queue after menu import or POS sync.
- [x] Begin with owner-selected important items.
- [x] Do not rank by sales before POS items reliably map to canonical menu items.
- [x] Distinguish recipe configured, purchase cost missing, partial cost, and complete cost.

## Verification expectations

Each phase must include tenant-isolation tests, API validation tests, frontend type checking, and focused workflow coverage. Migration changes must pass fresh-install, upgrade, and checksum-safety tests. Connector changes must preserve safe retries and avoid duplicated menu or sales records.

## Decisions requiring pilot evidence

- Which setup tasks restaurants will hand to Parline
- The strongest activation milestone for retention
- Which POS should follow Square
- How often supplier descriptions, SKUs, and pack sizes change
- How much historical invoice data should establish a silent baseline
- Whether operators will provide exact recipe quantities or prefer assisted migration
- Which recommendations genuinely require snooze
- Whether post-activation source health belongs primarily in Sources or selectively in Today
