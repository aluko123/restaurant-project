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
  onNavigate,
}: {
  request: ApiRequest;
  active: boolean;
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
    if (active) load();
  }, [active, load]);

  return (
    <section className="today-workspace" aria-labelledby="today-heading">
      <header className="today-heading">
        <p className="section-code">DB—TODAY</p>
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
          <h2>Nothing needs attention here right now.</h2>
          <p>Today only shows actions supported by current reviews, invoice price evidence, and completed inventory counts.</p>
          <button className="file-button" type="button" onClick={load}>Check again</button>
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
