import { FormEvent, useCallback, useEffect, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";

export type MigrationSetup = {
  posSystem: string | null;
  accountingSystem: string | null;
  completedAt: string | null;
  inventoryItemCount: number;
  menuItemCount: number;
  invoiceCount: number;
  salesDayCount: number;
  lastInvoiceAt: string | null;
  lastSalesDate: string | null;
  lastCompletedCountAt: string | null;
  lastMenuImportAt: string | null;
};

const posOptions = ["Toast", "Square", "Clover", "Lightspeed", "SpotOn", "TouchBistro", "Revel"];
const accountingOptions = [
  "QuickBooks Online",
  "QuickBooks Desktop",
  "Xero",
  "FreshBooks",
  "Spreadsheet or bookkeeper",
];

function toolHint(pos: string | null, accounting: string | null): string | null {
  const parts: string[] = [];
  const p = (pos ?? "").toLowerCase();
  const a = (accounting ?? "").toLowerCase();
  if (p.includes("toast")) {
    parts.push("Toast: Reports → Item Sales → export CSV for recent days.");
  } else if (p.includes("square")) {
    parts.push("Square: Reports → Sales → Items → export CSV.");
  } else if (p.includes("clover")) {
    parts.push("Clover: Reporting → Items → export a recent sales CSV.");
  } else if (pos) {
    parts.push(`${pos}: export item sales CSV if available, or enter a day manually.`);
  }
  if (a.includes("quickbooks") || a.includes("xero")) {
    parts.push("Keep accounting in place — Parline is for inventory and purchasing decisions.");
  }
  return parts.length ? parts.join(" ") : null;
}

function formatWhen(value: string | null, timezone?: string) {
  if (!value) return "Not yet";
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) {
    const date = new Date(`${value}T12:00:00Z`);
    return Number.isNaN(date.getTime())
      ? value
      : new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeZone: "UTC" }).format(date);
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  try {
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: timezone || undefined,
    }).format(date);
  } catch {
    return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(
      date,
    );
  }
}

function daysSince(value: string | null): number | null {
  if (!value) return null;
  const date = /^\d{4}-\d{2}-\d{2}$/.test(value)
    ? new Date(`${value}T12:00:00Z`)
    : new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return Math.floor((Date.now() - date.getTime()) / (24 * 60 * 60 * 1000));
}

function recencyLabel(value: string | null, unit: string) {
  if (!value) return `No ${unit} yet`;
  const days = daysSince(value);
  if (days === null) return formatWhen(value);
  if (days <= 0) return `Today`;
  if (days === 1) return `Yesterday`;
  return `${days} days ago`;
}

export function SourcesWorkspace({
  request,
  active,
  firstRun,
  onNavigate,
  onFinished,
}: {
  request: ApiRequest;
  active: boolean;
  firstRun: boolean;
  onNavigate: (path: string) => void;
  onFinished?: () => void;
}) {
  const [setup, setSetup] = useState<MigrationSetup | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [posSystem, setPosSystem] = useState("");
  const [accountingSystem, setAccountingSystem] = useState("");

  const load = useCallback(() => {
    setLoading(true);
    setError("");
    void request<MigrationSetup>("/v1/migration-setup")
      .then((next) => {
        setSetup(next);
        setPosSystem(next.posSystem ?? "");
        setAccountingSystem(next.accountingSystem ?? "");
      })
      .catch((cause: unknown) =>
        setError(cause instanceof Error ? cause.message : "Sources couldn't load. Try again."),
      )
      .finally(() => setLoading(false));
  }, [request]);

  useEffect(() => {
    if (active) load();
  }, [active, load]);

  async function saveTools(event: FormEvent) {
    event.preventDefault();
    if (!setup) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const next = await request<MigrationSetup>("/v1/migration-setup", {
        method: "PUT",
        body: JSON.stringify({
          posSystem: posSystem.trim() || null,
          accountingSystem: accountingSystem.trim() || null,
          markComplete: false,
        }),
      });
      setSetup(next);
      setNotice("Saved the tools you already use.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Those tools couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function finishSetup() {
    if (!setup) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const next = await request<MigrationSetup>("/v1/migration-setup", {
        method: "PUT",
        body: JSON.stringify({
          posSystem: posSystem.trim() || null,
          accountingSystem: accountingSystem.trim() || null,
          markComplete: true,
        }),
      });
      setSetup(next);
      setNotice("Setup finished for now. You can return here anytime.");
      onFinished?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Setup couldn't be finished. Try again.");
    } finally {
      setBusy(false);
    }
  }

  const total = setup
    ? setup.inventoryItemCount + setup.menuItemCount + setup.invoiceCount + setup.salesDayCount
    : 0;
  const hint = toolHint(setup?.posSystem ?? posSystem, setup?.accountingSystem ?? accountingSystem);
  const incomplete = Boolean(setup && !setup.completedAt);

  return (
    <section
      className={`sources-workspace${firstRun || incomplete ? " sources-first-run" : ""}`}
      aria-labelledby="sources-heading"
    >
      <header className="sources-heading">
        <p className="section-code">{incomplete ? "First shift setup" : "Your records"}</p>
        <h1 id="sources-heading">
          {incomplete ? "Start with what you already have." : "Sources"}
        </h1>
        <p>
          {incomplete
            ? "Connect what you have and upload what you can. You do not need every source before Parline becomes useful."
            : "Keep feeding Parline from the tools you already use. Fresh sources power better daily actions."}
        </p>
      </header>

      {loading ? (
        <p className="today-status" role="status">
          Loading your sources…
        </p>
      ) : error && !setup ? (
        <div className="today-load-error">
          <p className="form-error" role="alert">
            {error}
          </p>
          <button className="file-button" type="button" onClick={load}>
            Retry
          </button>
        </div>
      ) : setup ? (
        <>
          <form className="sources-tools" onSubmit={saveTools}>
            <div className="list-heading">
              <h2>Tools you already use</h2>
            </div>
            <p>Optional. Helps Parline point you at the right export — no forced integration.</p>
            <div className="inventory-form-fields">
              <label>
                POS <span>Optional</span>
                <select
                  value={posSystem}
                  onChange={(e) => setPosSystem(e.target.value)}
                >
                  <option value="">Not set</option>
                  {posOptions.map((option) => (
                    <option key={option} value={option}>
                      {option}
                    </option>
                  ))}
                  {posSystem && !posOptions.includes(posSystem) && (
                    <option value={posSystem}>{posSystem}</option>
                  )}
                </select>
              </label>
              <label>
                Accounting <span>Optional</span>
                <select
                  value={accountingSystem}
                  onChange={(e) => setAccountingSystem(e.target.value)}
                >
                  <option value="">Not set</option>
                  {accountingOptions.map((option) => (
                    <option key={option} value={option}>
                      {option}
                    </option>
                  ))}
                  {accountingSystem && !accountingOptions.includes(accountingSystem) && (
                    <option value={accountingSystem}>{accountingSystem}</option>
                  )}
                </select>
              </label>
            </div>
            {hint && <p className="setup-tools">{hint}</p>}
            <button className="file-button" type="submit" disabled={busy}>
              {busy ? "Saving…" : "Save tools"}
            </button>
          </form>

          <div className="today-setup-list sources-list">
            <SourceCard
              number="01"
              title="Inventory spreadsheet"
              description="Import item names, count units, categories, and pars, then begin the first physical count."
              countLabel={
                setup.inventoryItemCount
                  ? `${setup.inventoryItemCount} item${setup.inventoryItemCount === 1 ? "" : "s"}`
                  : "Import inventory"
              }
              complete={setup.inventoryItemCount > 0}
              recency={
                setup.lastCompletedCountAt
                  ? `Last count ${recencyLabel(setup.lastCompletedCountAt, "count")}`
                  : setup.inventoryItemCount > 0
                    ? "Ready for first count"
                    : "Not yet"
              }
              nudge={
                setup.inventoryItemCount > 0 && !setup.lastCompletedCountAt
                  ? "Start a physical count to unlock your first order guide."
                  : null
              }
              onClick={() => onNavigate("/inventory")}
            />
            <SourceCard
              number="02"
              title="Menu photo or PDF"
              description="Bring in the menu so sales can be matched to the items the restaurant actually sells."
              countLabel={
                setup.menuItemCount
                  ? `${setup.menuItemCount} item${setup.menuItemCount === 1 ? "" : "s"}`
                  : "Import menu"
              }
              complete={setup.menuItemCount > 0}
              recency={`Last import ${recencyLabel(setup.lastMenuImportAt, "import")}`}
              nudge={null}
              onClick={() => onNavigate("/menu")}
            />
            <SourceCard
              number="03"
              title="Supplier invoices"
              description="Upload recent invoices to seed suppliers, purchase prices, and product mappings."
              countLabel={
                setup.invoiceCount
                  ? `${setup.invoiceCount} invoice${setup.invoiceCount === 1 ? "" : "s"}`
                  : "Upload invoices"
              }
              complete={setup.invoiceCount > 0}
              recency={`Last upload ${recencyLabel(setup.lastInvoiceAt, "invoice")}`}
              nudge={
                setup.invoiceCount > 0 &&
                (daysSince(setup.lastInvoiceAt) ?? 99) > 14
                  ? "Upload recent invoices so prices stay current."
                  : setup.invoiceCount === 1
                    ? "A second invoice from the same supplier unlocks price tracking."
                    : null
              }
              onClick={() => onNavigate("/invoices")}
            />
            <SourceCard
              number="04"
              title="Recent sales"
              description="Import a complete POS sales day after menu items are available, or enter the day manually."
              countLabel={
                setup.salesDayCount
                  ? `${setup.salesDayCount} day${setup.salesDayCount === 1 ? "" : "s"}`
                  : "Import sales"
              }
              complete={setup.salesDayCount > 0}
              recency={`Last day ${recencyLabel(setup.lastSalesDate, "sales day")}`}
              nudge={
                (daysSince(setup.lastSalesDate) ?? 99) > 7
                  ? "Add recent sales to keep your weekly brief useful."
                  : null
              }
              onClick={() => onNavigate("/sales")}
            />
          </div>

          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
          {notice && (
            <p className="success-notice" role="status">
              {notice}
            </p>
          )}

          {incomplete ? (
            <div className="setup-finish">
              <p>
                {total
                  ? "You can finish setup now or add another source. Sources stays available anytime."
                  : "Add at least one source before finishing setup."}
              </p>
              <button
                className="ledger-button"
                type="button"
                disabled={busy || total === 0}
                onClick={() => void finishSetup()}
              >
                {busy ? "Finishing…" : "Finish setup for now"}
              </button>
            </div>
          ) : (
            <p className="sources-complete-note">
              Setup marked complete. Keep adding records here whenever something changes.
            </p>
          )}
        </>
      ) : null}
    </section>
  );
}

function SourceCard({
  number,
  title,
  description,
  countLabel,
  complete,
  recency,
  nudge,
  onClick,
}: {
  number: string;
  title: string;
  description: string;
  countLabel: string;
  complete: boolean;
  recency: string;
  nudge: string | null;
  onClick: () => void;
}) {
  return (
    <article className={`setup-action${complete ? " setup-action-complete" : ""}`}>
      <span className="setup-action-number">{complete ? "✓" : number}</span>
      <div className="setup-action-copy">
        <h3>{title}</h3>
        <p>{description}</p>
        <p className="source-recency">{recency}</p>
        {nudge && <p className="source-nudge">{nudge}</p>}
      </div>
      <button className={complete ? "text-button" : "file-button"} type="button" onClick={onClick}>
        {countLabel}
      </button>
    </article>
  );
}
