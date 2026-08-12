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

export function TodayWorkspace({
  request,
  active,
  canManageInvoices,
  setupIncomplete,
  onNavigate,
}: {
  request: ApiRequest;
  active: boolean;
  canManageInvoices: boolean;
  setupIncomplete: boolean;
  onNavigate: (path: string) => void;
}) {
  const [state, setState] = useState<TodayState>({ status: "loading" });

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
  }, [active, load]);

  return (
    <section className="today-workspace" aria-labelledby="today-heading">
      <header className="today-heading">
        <h1 id="today-heading">Today</h1>
        {state.status === "ready" ? (
          <p>
            {formatLocalDate(state.response.restaurantLocalDate)} · {state.response.timezone} ·
            Generated{" "}
            <time dateTime={state.response.generatedAt}>
              {formatTimestamp(state.response.generatedAt, state.response.timezone)}
            </time>
          </p>
        ) : (
          <p>A short, source-backed list of what needs attention.</p>
        )}
      </header>

      {canManageInvoices && setupIncomplete && (
        <aside className="today-setup-resume" aria-labelledby="today-setup-resume-heading">
          <div>
            <p className="section-code">Setup in progress</p>
            <h2 id="today-setup-resume-heading">Keep preparing your restaurant.</h2>
            <p>Continue the guided launch or self-service steps without losing anything already added.</p>
          </div>
          <button className="file-button" type="button" onClick={() => onNavigate("/sources")}>Continue setup</button>
        </aside>
      )}

      {state.status === "loading" ? (
        <p className="today-status" role="status">
          Checking current records…
        </p>
      ) : state.status === "error" ? (
        <div className="today-load-error">
          <p className="form-error" role="alert">
            {state.message}
          </p>
          <button className="file-button" type="button" onClick={load}>
            Retry Today
          </button>
        </div>
      ) : state.response.actions.length === 0 ? (
        <div className="today-empty">
          <p className="section-code">0 actions</p>
          <h2>No actions yet.</h2>
          <p>
            {canManageInvoices
              ? "Bring in a current record from Sources, then Parline can turn it into work for the next shift."
              : "Today builds this list from records prepared by your managers and from completed inventory counts. Open Inventory to see what is ready to count."}
          </p>
          <div className="today-empty-actions">
            {canManageInvoices ? (
              <button className="file-button" type="button" onClick={() => onNavigate("/sources")}>
                Open sources
              </button>
            ) : (
              <button className="file-button" type="button" onClick={() => onNavigate("/inventory")}>
                Open inventory counts
              </button>
            )}
          </div>
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
        <div>
          <dt>Why it matters</dt>
          <dd>{action.whyItMatters}</dd>
        </div>
        <div>
          <dt>Next action</dt>
          <dd>{action.nextAction}</dd>
        </div>
      </dl>
      <div className="today-evidence">
        <p className="section-code">Evidence</p>
        <strong>{action.evidence.value}</strong>
        <p>
          <time dateTime={action.evidence.timestamp}>
            {formatTimestamp(action.evidence.timestamp, timezone)}
          </time>
          {" · "}
          {action.evidence.source}
        </p>
        <p>
          <strong>Confidence:</strong> {action.confidence.reason}
        </p>
        <p>
          <strong>Limitation:</strong> {action.limitation}
        </p>
      </div>
      <button
        className="ledger-button today-target"
        type="button"
        onClick={() => onNavigate(action.target.path)}
      >
        {action.target.label}
        <span aria-hidden="true">→</span>
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
