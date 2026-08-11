import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";

export type MigrationSetup = {
  posSystem: string | null;
  accountingSystem: string | null;
  setupApproach: "assisted" | "self_service" | null;
  setupAssistanceRequestedAt: string | null;
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

type SquareConnection = {
  id: string;
  provider: string;
  status: string;
  lastSyncAt: string | null;
  lastSuccessAt: string | null;
  lastError: string | null;
  configured: boolean;
};

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
  const [squareConfigured, setSquareConfigured] = useState(false);
  const [squareConnection, setSquareConnection] = useState<SquareConnection | null>(null);
  const [squareSyncPending, setSquareSyncPending] = useState(false);
  const priorSquareStatus = useRef<string | null>(null);

  const load = useCallback((opts?: { quiet?: boolean }) => {
    if (!opts?.quiet) {
      setLoading(true);
      setError("");
    }
    return Promise.all([
      request<MigrationSetup>("/v1/migration-setup"),
      request<{ configured: boolean }>("/v1/connections/square/status").catch(() => ({
        configured: false,
      })),
      request<SquareConnection[]>("/v1/connections").catch(() => [] as SquareConnection[]),
    ])
      .then(([next, status, connections]) => {
        setSetup(next);
        setPosSystem(next.posSystem ?? "");
        setAccountingSystem(next.accountingSystem ?? "");
        setSquareConfigured(status.configured);
        const square = connections.find((c) => c.provider === "square") ?? null;
        setSquareConnection(square);
        return square;
      })
      .catch((cause: unknown) => {
        if (!opts?.quiet) {
          setError(cause instanceof Error ? cause.message : "Sources couldn't load. Try again.");
        }
        return null;
      })
      .finally(() => {
        if (!opts?.quiet) setLoading(false);
      });
  }, [request]);

  useEffect(() => {
    if (active) void load();
  }, [active, load]);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    if (!active) return;
    const params = new URLSearchParams(window.location.search);
    const square = params.get("square");
    if (square === "connected") {
      setNotice("Square authorized. Menu and sales are syncing in the background.");
      setSquareSyncPending(true);
      window.history.replaceState({}, "", "/sources");
      void load();
    } else if (square === "error") {
      setError(params.get("message") || "Square connection failed. Try again.");
      window.history.replaceState({}, "", "/sources");
    }
  }, [active, load]);

  // Poll while a Square sync is in flight so the button resets and status survives tab switches.
  useEffect(() => {
    if (!active) return;
    const status = squareConnection?.status ?? null;
    const shouldPoll = squareSyncPending || status === "syncing";
    if (!shouldPoll) return;

    let cancelled = false;
    let ticks = 0;
    const maxTicks = 40; // ~2 minutes at 3s
    const timer = window.setInterval(() => {
      if (cancelled) return;
      ticks += 1;
      void load({ quiet: true }).then((square) => {
        if (cancelled || !square) return;
        if (square.status === "syncing") return;
        setSquareSyncPending(false);
        if (square.status === "connected") {
          setNotice("Square sync finished. Check Menu and Sales for updates.");
          setError("");
        } else if (square.status === "error" || square.status === "needs_reauth") {
          setError(square.lastError || "Square sync failed. Try again or reconnect.");
          setNotice("");
        }
      });
      if (ticks >= maxTicks) {
        setSquareSyncPending(false);
        setNotice("Sync is taking longer than expected. Refresh Sources in a moment.");
        window.clearInterval(timer);
      }
    }, 3000);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [active, squareSyncPending, squareConnection?.status, load]);

  useEffect(() => {
    const prev = priorSquareStatus.current;
    const next = squareConnection?.status ?? null;
    if (
      prev === "syncing" &&
      next &&
      next !== "syncing" &&
      (next === "connected" || next === "error" || next === "needs_reauth")
    ) {
      setSquareSyncPending(false);
      if (next === "connected") {
        setNotice((current) => current || "Square sync finished. Check Menu and Sales for updates.");
      } else if (squareConnection?.lastError) {
        setError(squareConnection.lastError);
      }
    }
    priorSquareStatus.current = next;
  }, [squareConnection?.status, squareConnection?.lastError]);

  async function connectSquare() {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const { url } = await request<{ url: string }>("/v1/connections/square/authorize");
      window.location.href = url;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Square connect couldn't start.");
      setBusy(false);
    }
  }

  async function syncSquare() {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await request("/v1/connections/square/sync", { method: "POST", body: "{}" });
      setSquareSyncPending(true);
      setNotice("Square sync started. Menu and sales will update when it finishes.");
      const square = await load({ quiet: true });
      if (square && square.status !== "syncing" && square.status !== "connected") {
        // queued → may still be connected until worker claims
        setSquareConnection((current) =>
          current ? { ...current, status: "syncing" } : current,
        );
      }
    } catch (cause) {
      setSquareSyncPending(false);
      setError(cause instanceof Error ? cause.message : "Square sync couldn't start.");
    } finally {
      setBusy(false);
    }
  }

  async function disconnectSquare() {
    if (!window.confirm("Disconnect Square? Synced menu and sales stay in Parline.")) return;
    setBusy(true);
    setError("");
    try {
      await request("/v1/connections/square/disconnect", { method: "POST", body: "{}" });
      setNotice("Square disconnected.");
      setSquareConnection(null);
      load();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Square couldn't be disconnected.");
    } finally {
      setBusy(false);
    }
  }

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
          setupApproach: setup.setupApproach,
          markComplete: false,
        }),
      });
      setSetup(next);
      setNotice("Saved your menu and sales source.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "That source couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function chooseApproach(approach: "assisted" | "self_service") {
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
          setupApproach: approach,
          markComplete: false,
        }),
      });
      setSetup(next);
      setNotice(approach === "assisted" ? "Setup help requested." : "Self-service setup selected.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Your setup path couldn't be saved.");
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
          setupApproach: setup.setupApproach,
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
          <button className="file-button" type="button" onClick={() => load()}>
            Retry
          </button>
        </div>
      ) : setup ? (
        <>
          {incomplete && !setup.setupApproach ? (
            <SetupApproachChoice busy={busy} onChoose={chooseApproach} />
          ) : <>
          {incomplete && setup.setupApproach && (
            <section className="setup-path-summary" aria-labelledby="setup-path-heading">
              <p className="section-code">{setup.setupApproach === "assisted" ? "Setup help requested" : "You are leading setup"}</p>
              <h2 id="setup-path-heading">{setup.setupApproach === "assisted" ? "Launch with Parline" : "Self-service setup"}</h2>
              <p>{setup.setupApproach === "assisted" ? "Your request is saved for the Parline team. We’ll coordinate the migration using your account details, prepare imports and mappings, and bring back only decisions that need restaurant knowledge." : "You are migrating the restaurant’s records. Connect or import each source below, review the results, and prepare the first count at your own pace."}</p>
              <ol className="setup-path-steps">
                {setup.setupApproach === "assisted" ? <>
                  <li><strong>Parline coordinates the handoff</strong><span>We connect with you about the files and systems you already use.</span></li>
                  <li><strong>You authorize and share</strong><span>You approve external connections and provide records; credentials stay with you.</span></li>
                  <li><strong>Parline prepares, you confirm</strong><span>We organize the migration and ask you only about uncertain mappings or physical counts.</span></li>
                </> : <>
                  <li><strong>Choose each source</strong><span>Connect Square or use the guided export path for another POS.</span></li>
                  <li><strong>Import and review</strong><span>You upload records and resolve any items that need confirmation.</span></li>
                  <li><strong>Start the first count</strong><span>You verify what is physically on hand before relying on inventory actions.</span></li>
                </>}
              </ol>
              <button className="text-button" type="button" disabled={busy} onClick={() => void chooseApproach(setup.setupApproach === "assisted" ? "self_service" : "assisted")}>{setup.setupApproach === "assisted" ? "Cancel help and set it up myself" : "Request setup help"}</button>
            </section>
          )}
          {(posSystem === "Square" || Boolean(squareConnection)) && <section className="sources-tools square-connect-panel" aria-labelledby="square-connect-heading">
            <div className="list-heading">
              <h2 id="square-connect-heading">Square</h2>
            </div>
            {squareConfigured ? (
              squareConnection && squareConnection.status !== "disconnected" ? (
                <>
                  <p>
                    Status: <strong>{squareStatusLabel(squareConnection.status)}</strong>
                    {squareConnection.lastSuccessAt
                      ? ` · Last successful sync ${recencyLabel(squareConnection.lastSuccessAt, "sync")}`
                      : squareConnection.lastSyncAt
                        ? ` · Last attempt ${recencyLabel(squareConnection.lastSyncAt, "sync")}`
                        : " · Waiting for first sync"}
                  </p>
                  {squareConnection.lastError && (
                    <p className="form-error" role="alert">
                      {squareConnection.lastError}
                    </p>
                  )}
                  <p>
                    {squareConnection.status === "error" || squareConnection.status === "needs_reauth"
                      ? "Square needs attention before menu and sales can update again. Synced records stay in Parline."
                      : "Square fills menu items and recent sales automatically. Inventory counts and supplier invoices stay in Parline."}
                  </p>
                  <div className="card-actions">
                    {squareConnection.status !== "needs_reauth" && squareConnection.status !== "error" && <button
                      className="ledger-button"
                      type="button"
                      disabled={
                        busy ||
                        squareSyncPending ||
                        squareConnection.status === "syncing"
                      }
                      onClick={() => void syncSquare()}
                    >
                      {busy ||
                      squareSyncPending ||
                      squareConnection.status === "syncing"
                        ? "Syncing…"
                        : "Sync now"}
                    </button>}
                    {(squareConnection.status === "needs_reauth" ||
                      squareConnection.status === "error") && (
                      <button
                        className="ledger-button"
                        type="button"
                        disabled={busy}
                        onClick={() => void connectSquare()}
                      >
                        Reconnect
                      </button>
                    )}
                    <button
                      className="text-button"
                      type="button"
                      disabled={busy}
                      onClick={() => void disconnectSquare()}
                    >
                      Disconnect
                    </button>
                  </div>
                </>
              ) : (
                <>
                  <p>
                    Connect Square to pull your menu and about 90 days of sales — no CSV export
                    required.
                  </p>
                  <button
                    className="ledger-button"
                    type="button"
                    disabled={busy}
                    onClick={() => void connectSquare()}
                  >
                    {busy ? "Opening Square…" : "Connect Square"}
                  </button>
                </>
              )
            ) : (
              <p>
                Square connect is not configured on this server yet. You can still import menu and
                sales with CSV, or ask for setup help.
              </p>
            )}
          </section>}

          <form className="sources-tools" onSubmit={saveTools}>
            <div className="list-heading">
              <h2>Menu and sales source</h2>
            </div>
            <p>
              Choose the POS or ordering system that holds your menu and sales. Square can connect
              directly; other systems use a guided export for now.
            </p>
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
            </div>
            <button className="file-button" type="submit" disabled={busy}>
              {busy ? "Saving…" : "Save source"}
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
              onClick={() => onNavigate("/sales")}
            />
          </div>

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
          </>}
          {error && (
            <p className="form-error sources-message" role="alert">
              {error}
            </p>
          )}
          {notice && (
            <p className="success-notice sources-message" role="status">
              {notice}
            </p>
          )}
        </>
      ) : null}
    </section>
  );
}

function SetupApproachChoice({ busy, onChoose }: { busy: boolean; onChoose: (approach: "assisted" | "self_service") => Promise<void> }) {
  return <section className="setup-approach" aria-labelledby="setup-approach-heading">
    <p className="section-code">Choose how to launch</p>
    <h2 id="setup-approach-heading">Get your restaurant ready for Parline.</h2>
    <div className="setup-approach-options">
      <article className="setup-approach-card setup-approach-recommended">
        <p className="invoice-status">Recommended</p>
        <h3>Launch with Parline</h3>
        <p>Request a setup handoff with the Parline team. We coordinate with you, prepare imports and mappings, and get the first count ready. You authorize accounts and confirm restaurant-specific decisions.</p>
        <button className="ledger-button" type="button" disabled={busy} onClick={() => void onChoose("assisted")}>{busy ? "Sending request…" : "Request guided setup"}</button>
      </article>
      <article className="setup-approach-card">
        <h3>Set it up myself</h3>
        <p>Migrate your own records with a guided checklist. You connect or import each source, review mappings, and prepare the first count. Pause anytime or request help later without starting over.</p>
        <button className="file-button" type="button" disabled={busy} onClick={() => void onChoose("self_service")}>{busy ? "Saving…" : "Use self-service setup"}</button>
      </article>
    </div>
  </section>;
}

function squareStatusLabel(status: string) {
  switch (status) {
    case "connected":
      return "Connected";
    case "syncing":
      return "Syncing";
    case "needs_reauth":
      return "Needs reconnect";
    case "error":
      return "Error";
    case "pending":
      return "Pending";
    default:
      return status;
  }
}

function SourceCard({
  number,
  title,
  description,
  countLabel,
  complete,
  recency,
  onClick,
}: {
  number: string;
  title: string;
  description: string;
  countLabel: string;
  complete: boolean;
  recency: string;
  onClick: () => void;
}) {
  return (
    <article className={`setup-action${complete ? " setup-action-complete" : ""}`}>
      <span className="setup-action-number">{complete ? "✓" : number}</span>
      <div className="setup-action-copy">
        <h3>{title}</h3>
        <p>{description}</p>
        <p className="source-recency">{recency}</p>
      </div>
      <button className={complete ? "text-button" : "file-button"} type="button" onClick={onClick}>
        {countLabel}
      </button>
    </article>
  );
}
