import { useCallback, useEffect, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";

type Priority = "urgent" | "high" | "normal";
type ConfidenceLevel = "high" | "medium";

type TodayAction = {
  actionId: string;
  ruleKey: string;
  category: string;
  priority: Priority;
  confidence: { level: ConfidenceLevel; reason: string };
  title: string;
  whyItMatters: string;
  nextAction: string;
  evidence: { timestamp: string; value: string; source: string };
  limitation: string;
  target: { workspace: string; path: string; label: string };
};

type TodayResponse = {
  timezone: string;
  restaurantLocalDate: string;
  generatedAt: string;
  actions: TodayAction[];
};

type TodayState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; response: TodayResponse };

type MigrationSetup = {
  posSystem: string | null;
  accountingSystem: string | null;
  completedAt: string | null;
  inventoryItemCount: number;
  menuItemCount: number;
  invoiceCount: number;
  salesDayCount: number;
};

export function TodayWorkspace({
  request,
  active,
  canManageInvoices,
  onNavigate,
}: {
  request: ApiRequest;
  active: boolean;
  canManageInvoices: boolean;
  onNavigate: (path: string) => void;
}) {
  const [state, setState] = useState<TodayState>({ status: "loading" });
  const [setup, setSetup] = useState<MigrationSetup | null>(null);
  const [finishingSetup, setFinishingSetup] = useState(false);
  const [setupError, setSetupError] = useState("");

  const load = useCallback(() => {
    setState({ status: "loading" });
    void request<TodayResponse>("/v1/today")
      .then((response) => setState({ status: "ready", response }))
      .catch((cause: unknown) =>
        setState({
          status: "error",
          message:
            cause instanceof Error
              ? cause.message
              : "Today's actions couldn't load. Please try again.",
        }),
      );
  }, [request]);

  useEffect(() => {
    if (!active) return;
    load();
    if (canManageInvoices) void request<MigrationSetup>("/v1/migration-setup").then(setSetup).catch(()=>setSetup(null));
  }, [active, canManageInvoices, load, request]);

  async function finishSetup() {
    if (!setup) return;
    setFinishingSetup(true);setSetupError("");
    try { setSetup(await request<MigrationSetup>("/v1/migration-setup",{method:"PUT",body:JSON.stringify({posSystem:setup.posSystem,accountingSystem:setup.accountingSystem,markComplete:true})})); }
    catch (cause) { setSetupError(cause instanceof Error?cause.message:"Setup couldn't be finished. Try again."); }
    finally { setFinishingSetup(false); }
  }

  return (
    <section className="today-workspace" aria-labelledby="today-heading">
      <header className="today-heading">
        <h1 id="today-heading">Today</h1>
        {state.status === "ready" ? (
          <p>
            {formatLocalDate(state.response.restaurantLocalDate)} · {state.response.timezone} · Generated{" "}
            <time dateTime={state.response.generatedAt}>
              {formatTimestamp(state.response.generatedAt, state.response.timezone)}
            </time>
          </p>
        ) : (
          <p>A short, source-backed list of what needs attention.</p>
        )}
      </header>

      {canManageInvoices&&setup&&!setup.completedAt&&(
        <MigrationSetupGuide setup={setup} busy={finishingSetup} error={setupError} onNavigate={onNavigate} onFinish={()=>void finishSetup()}/>
      )}

      {state.status === "loading" ? (
        <p className="today-status" role="status">Checking current records…</p>
      ) : state.status === "error" ? (
        <div className="today-load-error">
          <p className="form-error" role="alert">{state.message}</p>
          <button className="file-button" type="button" onClick={load}>Retry Today</button>
        </div>
      ) : state.response.actions.length === 0 ? (
        <div className="today-empty">
          <p className="section-code">0 actions</p>
          <h2>No actions yet.</h2>
          <p>{canManageInvoices ? "Bring in one current record above, then Parline can begin turning it into work for the next shift." : "Today builds this list from records prepared by your managers and from completed inventory counts. Open Inventory to see what is ready to count."}</p>
          {!canManageInvoices&&<div className="today-empty-actions"><button className="file-button" type="button" onClick={() => onNavigate("/inventory")}>Open inventory counts</button></div>}
        </div>
      ) : (
        <div className="today-actions" aria-label={`${state.response.actions.length} actions`}>
          {state.response.actions.map((action, index) => (
            <TodayActionCard
              key={action.actionId}
              action={action}
              index={index}
              timezone={state.response.timezone}
              onNavigate={onNavigate}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function MigrationSetupGuide({setup,busy,error,onNavigate,onFinish}:{setup:MigrationSetup;busy:boolean;error:string;onNavigate:(path:string)=>void;onFinish:()=>void}) {
  const total=setup.inventoryItemCount+setup.menuItemCount+setup.invoiceCount+setup.salesDayCount;
  return <section className="migration-setup" aria-labelledby="migration-setup-heading"><p className="section-code">Bring your records</p><h2 id="migration-setup-heading">Start with what you already have.</h2><p>Connect what you have and upload what you can. You do not need every source before Parline becomes useful.</p>{(setup.posSystem||setup.accountingSystem)&&<p className="setup-tools">Existing tools: {[setup.posSystem,setup.accountingSystem].filter(Boolean).join(" · ")}</p>}<div className="today-setup-list"><SetupAction number="01" title="Inventory spreadsheet" description="Import item names, count units, categories, and pars, then begin the first physical count." action={setup.inventoryItemCount?`${setup.inventoryItemCount} added`:"Import inventory"} complete={setup.inventoryItemCount>0} onClick={() => onNavigate("/inventory")}/><SetupAction number="02" title="Menu photo or PDF" description="Bring in the menu so sales can be matched to the items the restaurant actually sells." action={setup.menuItemCount?`${setup.menuItemCount} added`:"Import menu"} complete={setup.menuItemCount>0} onClick={() => onNavigate("/menu")}/><SetupAction number="03" title="Supplier invoices" description="Upload recent invoices to seed suppliers, purchase prices, and product mappings." action={setup.invoiceCount?`${setup.invoiceCount} uploaded`:"Upload invoices"} complete={setup.invoiceCount>0} onClick={() => onNavigate("/invoices")}/><SetupAction number="04" title="Recent sales" description="Import a complete POS sales day after menu items are available, or enter the day manually." action={setup.salesDayCount?`${setup.salesDayCount} days added`:"Import sales"} complete={setup.salesDayCount>0} onClick={() => onNavigate("/sales")}/></div>{error&&<p className="form-error" role="alert">{error}</p>}<div className="setup-finish"><p>{total?"You can finish setup now or add another source.":"Add at least one source before finishing setup."}</p><button className="ledger-button" type="button" disabled={busy||total===0} onClick={onFinish}>{busy?"Finishing…":"Finish setup for now"}</button></div></section>;
}

function SetupAction({number,title,description,action,complete,onClick}:{number:string;title:string;description:string;action:string;complete:boolean;onClick:()=>void}) {
  return <article className={`setup-action${complete?" setup-action-complete":""}`}><span className="setup-action-number">{complete?"✓":number}</span><div className="setup-action-copy"><h3>{title}</h3><p>{description}</p></div><button className={complete?"text-button":"file-button"} type="button" onClick={onClick}>{action}</button></article>;
}

function TodayActionCard({
  action,
  index,
  timezone,
  onNavigate,
}: {
  action: TodayAction;
  index: number;
  timezone: string;
  onNavigate: (path: string) => void;
}) {
  return (
    <article className={`today-action today-priority-${action.priority}`}>
      <div className="today-action-head">
        <span className="today-action-number">{String(index + 1).padStart(2, "0")}</span>
        <div className="today-badges">
          <span>{priorityLabel(action.priority)} priority</span>
          <span>{action.confidence.level} confidence</span>
        </div>
      </div>
      <h2>{action.title}</h2>
      <dl className="today-action-copy">
        <div><dt>Why it matters</dt><dd>{action.whyItMatters}</dd></div>
        <div><dt>Next action</dt><dd>{action.nextAction}</dd></div>
      </dl>
      <div className="today-evidence">
        <p className="section-code">Evidence</p>
        <strong>{action.evidence.value}</strong>
        <p>
          <time dateTime={action.evidence.timestamp}>
            {formatTimestamp(action.evidence.timestamp, timezone)}
          </time>
          {" · "}{action.evidence.source}
        </p>
        <p><strong>Confidence:</strong> {action.confidence.reason}</p>
        <p><strong>Limitation:</strong> {action.limitation}</p>
      </div>
      <button className="ledger-button today-target" type="button" onClick={() => onNavigate(action.target.path)}>
        {action.target.label}<span aria-hidden="true">→</span>
      </button>
    </article>
  );
}

function priorityLabel(priority: Priority) {
  return priority.charAt(0).toUpperCase() + priority.slice(1);
}

function formatLocalDate(value: string) {
  const date = new Date(`${value}T12:00:00Z`);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeZone: "UTC" }).format(date);
}

function formatTimestamp(value: string, timezone: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  try {
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: timezone,
    }).format(date);
  } catch {
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
      timeZone: "UTC",
    }).format(date);
  }
}
