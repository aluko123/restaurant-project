---
name: designing-restaurant-saas-ux
description: Designs and reviews mobile-first UI/UX for the restaurant SaaS web app. Use when planning, building, or reviewing onboarding, dashboards, invoice upload, inventory counts, recommendations, reports, accessibility, UX writing, React UI, or Rust-backed product flows.
license: MIT
---

# Designing Restaurant SaaS UX

Use this skill to keep the restaurant SaaS product clear, mobile-first, accessible, and operator-focused while building a web app with a Rust-oriented backend.

## Product North Star

The product is a daily profit copilot for independent restaurants:

> Keep your POS. Snap invoices. Know what to buy, what to prep, and where profit is leaking.

Design every screen around one question: **what should the restaurant owner or manager do next?**

Avoid designing a generic ERP, accounting suite, POS replacement, or analytics dashboard. The app should feel like a trusted assistant that turns messy restaurant data into a few clear daily actions.

## Primary Users

1. **Owner/operator**
   - Wants profit clarity, purchasing guidance, price-change alerts, and weekly summaries.
   - Has little patience for complex setup or dashboards.
   - Needs confidence, explanations, and ROI.

2. **Manager/kitchen lead**
   - Uploads invoices, counts key items, logs waste/stockouts, reviews prep/order suggestions.
   - Needs fast mobile workflows usable during a busy shift.

3. **Staff helper**
   - Performs narrow tasks only: upload receipt, count inventory, log waste.
   - Should not see complex analytics or sensitive financials.

4. **Bookkeeper/accountant**
   - Needs exports, invoice summaries, expense categories, and clean supporting data.

## UX Principles

### Action Over Dashboard

Prefer recommendations and tasks over passive charts.

- Bad: “Food cost variance: 4.8%.”
- Good: “Chicken cost rose 11%. Your chicken tacos may need a price or portion review.”
- Bad: “Inventory risk detected.”
- Good: “You may run out of tortillas before Saturday dinner. Suggested order: 6 cases.”

Every alert should include:

1. What changed.
2. Why it matters.
3. Recommended next action.
4. Confidence or uncertainty when forecasting.

### Mobile First, Kitchen Friendly

Assume users are on phones in noisy, rushed restaurant environments.

- Use large tap targets: aim for 44px minimum.
- Minimize typing; prefer camera upload, pickers, saved defaults, and one-tap choices.
- Make primary actions visible without hunting.
- Keep forms short and resumable.
- Use plain language that is readable at a glance.
- Support glare/low-light use with strong contrast.
- Avoid dense tables on mobile; use cards or progressive disclosure.

### “Top 20” Before “Track Everything”

Do not force full ERP inventory behavior.

Prioritize high-impact items:

- High cost.
- High waste risk.
- Frequent stockouts.
- Ingredients tied to top-selling menu items.
- Packaging or supplies that stop service when missing.

Default to “track the important items first” and let deeper inventory come later.

### Keep Existing Tools

Never imply that users must replace Clover, Toast, Square, QuickBooks, Excel, paper, or WhatsApp to get value.

Preferred framing:

- “Connect or upload your sales.”
- “Forward or snap invoices.”
- “Export a clean summary for your bookkeeper.”
- “Keep your POS; use this for profit and purchasing decisions.”

### Progressive Disclosure

Show the simple answer first, then details.

Recommendation card structure:

1. Headline: “Order 35 lb chicken before Friday.”
2. Reason: “Weekend sales are trending 12% above normal and current stock is low.”
3. Confidence: “Medium confidence.”
4. Details toggle: sales history, invoice history, current stock, weather/event factors.
5. Actions: Approve, Edit, Dismiss, Mark as ordered.

## Core MVP Screens

Prioritize these screens and flows before broader ERP features:

1. **Onboarding**
   - Restaurant basics, location, cuisine/service style, POS/tool usage, suppliers, top menu items, top ingredients.
   - Avoid asking for every recipe at signup.

2. **Home / Today**
   - Today’s 3–5 actions.
   - Inventory risks.
   - Supplier price changes.
   - Suggested order/prep tasks.
   - Missing data prompts.

3. **Invoice Upload**
   - Camera/photo/PDF upload.
   - Show extraction status.
   - Ask only for low-confidence corrections.
   - Highlight vendor price changes after processing.

4. **Inventory Count**
   - Count only tracked items by default.
   - Support units like lb, case, bag, bottle, gallon, each.
   - Allow “low / okay / full” mode for early pilots.
   - Save drafts and reduce repeated input.

5. **Waste and Stockout Log**
   - Fast reason selection.
   - Optional photo/note.
   - Estimate cost/lost sales when possible.

6. **Order / Prep Recommendations**
   - Suggested quantity.
   - Risk of stockout or over-ordering.
   - Explanation and confidence.
   - Approve/edit feedback loop.

7. **Menu Margin Snapshot**
   - Start with top menu items only.
   - Show approximate margin trend, not false precision.
   - Recommend reprice, promote, portion review, or remove.

8. **Weekly Profit Snapshot**
   - Sales, purchases, estimated food cost, waste, price movers, stockouts, next-week recommendations.
   - Exportable PDF/CSV for owner/bookkeeper.

## Visual Design Direction

Use a practical, trustworthy SaaS style rather than flashy restaurant imagery.

Recommended tone:

- Clear.
- Calm.
- Operational.
- Warm enough for independent restaurants.
- Premium enough to earn trust with financial data.

Suggested visual system:

- Neutral base palette with strong contrast.
- One primary brand color for action.
- Semantic status colors: risk, warning, success, info.
- Generous spacing and clear hierarchy.
- Rounded but not childish components.
- Charts only when they support a decision.
- Subtle motion for state changes, not decorative animation.

Avoid:

- Generic purple SaaS gradients everywhere.
- Tiny gray text.
- Dashboard walls of charts.
- Dense spreadsheet UI on mobile.
- Overuse of red alerts that cause fatigue.
- Restaurant stock photography as a substitute for product clarity.

## Accessibility Baseline

Build accessibility in from the first component.

Minimum expectations:

- Semantic HTML for forms, buttons, navigation, headings, and tables.
- Visible focus states for all interactive controls.
- Keyboard-accessible dialogs, menus, tabs, and upload flows.
- Labels and descriptions for every input.
- Error messages associated with fields.
- Color contrast meeting WCAG AA: generally 4.5:1 for text.
- Do not rely on color alone for status; pair with icons/text.
- Respect reduced motion preferences.
- Use accessible names for icon-only buttons.
- Preserve logical heading order and landmark regions.

Restaurant-specific accessibility:

- Support older users and stressed users with plain language and forgiving flows.
- Make destructive actions confirmable and reversible where possible.
- Use readable numbers, units, and dates.
- Make uploaded invoice review possible on small screens.

## UX Writing Rules

Use restaurant-owner language, not enterprise jargon.

Prefer:

- “You may run out of chicken by Saturday dinner.”
- “Tomatoes cost 14% more than your last order.”
- “Save today’s count.”
- “Mark as ordered.”
- “Send to bookkeeper.”

Avoid:

- “Anomaly detected.”
- “COGS optimization opportunity.”
- “Submit payload.”
- “Variance exception generated.”
- “Theoretical usage discrepancy.”

Error messages should explain recovery:

- Bad: “Upload failed.”
- Good: “Invoice upload failed. Try again, or upload a PDF/photo from your files.”

Empty states should teach the next step:

- “No invoices yet. Upload your first supplier invoice to start tracking price changes.”

## React / Frontend Engineering Guidance

If the frontend uses React or Next.js:

- Keep components small and purpose-driven.
- Avoid large components with many boolean props; prefer composition or clear variants.
- Centralize reusable UI primitives: Button, Card, Alert, Input, Select, UploadPanel, MetricCard, RecommendationCard.
- Use semantic component APIs tied to product meaning, not visual hacks.
- Keep loading, empty, error, and success states explicit.
- Avoid premature animation libraries; use CSS transitions first.
- Avoid data-fetching waterfalls; design pages around server/client boundaries deliberately.
- Prefer accessible component libraries or patterns when they reduce risk.
- Ensure mobile layout is not an afterthought; implement mobile before desktop polish.

## Rust-Oriented Product Architecture Guidance

Rust is a strong fit for the backend/API, ingestion, data processing, and forecasting jobs. Use it to get reliability, performance, and type safety.

Default stance:

- Prefer safe Rust.
- Avoid `unsafe` unless there is a measured, specific need and the invariants are documented.
- Use explicit domain types for money, units, quantities, suppliers, invoices, menu items, and locations.
- Treat unit conversion, currency, and decimal math carefully; avoid floating-point money calculations.
- Validate at API boundaries and preserve clear error messages for the UI.
- Design APIs around user workflows, not database tables.

Good Rust-backed product boundaries:

- Auth, tenants, roles, restaurants, locations.
- Invoice ingestion and extraction review state.
- Inventory item catalog and count sessions.
- Sales imports and POS integration jobs.
- Recommendation generation and explanation storage.
- Reports/exports.

Do not let backend complexity leak into UX. For example, users should see “Invoice needs review,” not “OCR parse confidence below threshold in line item normalization job.”

## Design Review Checklist

Before considering a screen done, check:

1. Can a busy owner understand the value in 5 seconds?
2. Is the next action obvious?
3. Does the mobile layout work first?
4. Are tap targets large enough?
5. Is there a clear empty/loading/error/success state?
6. Does every alert include a recommended action?
7. Is language plain and restaurant-specific?
8. Are forms labeled and keyboard accessible?
9. Does color contrast meet WCAG AA?
10. Does the screen avoid unnecessary ERP/accounting jargon?
11. Can staff complete their task without seeing irrelevant analytics?
12. Does the design support later exports/bookkeeper workflows without making MVP heavy?

## MVP Scope Guardrails

Build now:

- Mobile-first web app/PWA foundation.
- Restaurant onboarding.
- Sales import/manual entry.
- Invoice upload and review.
- Supplier/item price alerts.
- Top ingredient tracking.
- Simple inventory count.
- Waste/stockout log.
- Daily actions.
- Weekly snapshot.
- Export basics.

Defer:

- POS replacement.
- Payment processing.
- Payroll.
- Full accounting/general ledger.
- Full AP/bill pay.
- Autonomous purchasing.
- Complete recipe database for every item.
- Enterprise multi-location controls beyond basic future-proofing.

## Output Expectations When Using This Skill

When designing or reviewing, return concrete artifacts:

- Screen goals and primary user.
- Key user flow.
- Component/state list.
- Mobile-first layout notes.
- Copy examples.
- Accessibility considerations.
- Backend/API implications, especially Rust domain concerns.
- Scope cuts if the design is becoming too complex.

When implementing, favor the smallest useful slice that proves daily value for a pilot restaurant.
