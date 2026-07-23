import { useCallback, useEffect, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";

type CurrencyAmount = { currency: string; amount: string };
type EnteredQuantity = {
  menuItemId: string;
  menuItemName: string;
  enteredQuantity: string;
};
type CountGroup = {
  inventoryItemId: string;
  inventoryItemName: string;
  eventCount: number;
};
type ReasonCount = { reason: string; eventCount: number };
type WasteQuantity = {
  inventoryItemId: string;
  inventoryItemName: string;
  countUnit: string;
  enteredQuantity: string;
  eventCount: number;
  lastLoggedAt: string;
};

type WeeklyBriefResponse = {
  timezone: string;
  weekStart: string;
  weekEnd: string;
  utcStart: string;
  utcEnd: string;
  generatedAt: string;
  isLivePreview: true;
  daysElapsed: number;
  caveats: string[];
  sales: {
    daysWithData: number;
    daysElapsed: number;
    daysInWeek: number;
    reportedLineCount: number;
    linesWithoutReportedSales: number;
    enteredSalesByCurrency: CurrencyAmount[];
    topItemsByEnteredQuantity: EnteredQuantity[];
    caveats: string[];
  };
  purchases: {
    receiptCount: number;
    usablePositiveLineTotalCount: number;
    linesMissingOrNonpositiveTotalCount: number;
    recordedInvoiceLinePurchasesByCurrency: CurrencyAmount[];
    caveats: string[];
  };
  losses: {
    wasteCount: number;
    stockoutCount: number;
    wasteAffectedItems: CountGroup[];
    stockoutAffectedItems: CountGroup[];
    wasteReasons: ReasonCount[];
    stockoutReasons: ReasonCount[];
    recentWasteQuantities: WasteQuantity[];
    caveats: string[];
  };
};

type BriefState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; response: WeeklyBriefResponse };

export function WeeklyBriefWorkspace({
  request,
  active,
}: {
  request: ApiRequest;
  active: boolean;
}) {
  const [state, setState] = useState<BriefState>({ status: "loading" });

  const load = useCallback(() => {
    setState({ status: "loading" });
    void request<WeeklyBriefResponse>("/v1/weekly-brief")
      .then((response) => setState({ status: "ready", response }))
      .catch((cause: unknown) =>
        setState({
          status: "error",
          message:
            cause instanceof Error
              ? cause.message
              : "The current-week brief couldn't load. Please try again.",
        }),
      );
  }, [request]);

  useEffect(() => {
    if (active) load();
  }, [active, load]);

  return (
    <section className="weekly-brief-workspace" aria-labelledby="weekly-brief-heading">
      <header className="weekly-brief-heading">
        <div className="weekly-brief-eyebrow">
          <p className="section-code">DB—WEEKLY BRIEF</p>
          <span>Live preview</span>
        </div>
        <h1 id="weekly-brief-heading">Current week</h1>
        {state.status === "ready" ? (
          <div className="weekly-brief-dates">
            <p>{formatWeekRange(state.response.weekStart, state.response.weekEnd)}</p>
            <p>
              {state.response.timezone} · Day {state.response.daysElapsed} of 7 · Generated{" "}
              <time dateTime={state.response.generatedAt}>
                {formatTimestamp(state.response.generatedAt, state.response.timezone)}
              </time>
            </p>
          </div>
        ) : (
          <p>A read-only view of facts entered for this restaurant's current local week.</p>
        )}
      </header>

      {state.status === "loading" ? (
        <p className="weekly-brief-status" role="status">Loading current-week records…</p>
      ) : state.status === "error" ? (
        <div className="weekly-brief-error">
          <p className="form-error" role="alert">{state.message}</p>
          <button className="file-button" type="button" onClick={load}>Retry brief</button>
        </div>
      ) : (
        <BriefContents response={state.response} />
      )}
    </section>
  );
}

function BriefContents({ response }: { response: WeeklyBriefResponse }) {
  const { sales, purchases, losses } = response;
  return (
    <>
      <div className="weekly-brief-disclosure">
        <strong>Data completeness</strong>
        <p>
          Entered sales exist for {sales.daysWithData} {plural(sales.daysWithData, "business day")}.
          The restaurant is on day {sales.daysElapsed} of this {sales.daysInWeek}-day local week.
        </p>
        {response.caveats.map((caveat) => <p key={caveat}>{caveat}</p>)}
      </div>

      <div className="weekly-brief-sections">
        <BriefSection code="01 / SALES" title="Entered sales" caveats={sales.caveats}>
          <div className="brief-stat-grid">
            <BriefStat value={String(sales.daysWithData)} label="Days with entered sales" />
            <BriefStat value={String(sales.reportedLineCount)} label="Lines with reported net sales" />
            <BriefStat value={String(sales.linesWithoutReportedSales)} label="Lines without reported sales" />
          </div>
          <BriefBlock title="Entered sales by currency">
            {sales.enteredSalesByCurrency.length ? (
              <CurrencyList values={sales.enteredSalesByCurrency} />
            ) : (
              <p className="brief-empty">No reported net sales amounts were entered on this week's sales lines.</p>
            )}
          </BriefBlock>
          <BriefBlock title="Menu items by entered quantity">
            {sales.topItemsByEnteredQuantity.length ? (
              <ol className="brief-ranked-list">
                {sales.topItemsByEnteredQuantity.map((item) => (
                  <li key={item.menuItemId}>
                    <span>{item.menuItemName}</span>
                    <strong>{formatDecimal(item.enteredQuantity)} entered</strong>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="brief-empty">No menu-item quantities were entered for this week.</p>
            )}
          </BriefBlock>
        </BriefSection>

        <BriefSection code="02 / PURCHASES" title="Recorded purchases" caveats={purchases.caveats}>
          <div className="brief-stat-grid">
            <BriefStat value={String(purchases.receiptCount)} label="Recorded receipts" />
            <BriefStat value={String(purchases.usablePositiveLineTotalCount)} label="Positive saved line totals" />
            <BriefStat value={String(purchases.linesMissingOrNonpositiveTotalCount)} label="Excluded line totals" />
          </div>
          <BriefBlock title="Recorded invoice-line purchases by currency">
            {purchases.recordedInvoiceLinePurchasesByCurrency.length ? (
              <CurrencyList values={purchases.recordedInvoiceLinePurchasesByCurrency} />
            ) : (
              <p className="brief-empty">No positive saved invoice-line totals were recorded for this week.</p>
            )}
          </BriefBlock>
        </BriefSection>

        <BriefSection code="03 / LOSSES" title="Waste & stockouts" caveats={losses.caveats}>
          <div className="brief-stat-grid brief-loss-counts">
            <BriefStat value={String(losses.wasteCount)} label="Waste logs" />
            <BriefStat value={String(losses.stockoutCount)} label="Stockout logs" />
          </div>
          {losses.wasteCount + losses.stockoutCount === 0 ? (
            <p className="brief-empty">No waste or stockouts were logged during this local week.</p>
          ) : (
            <div className="brief-loss-grid">
              <LossColumn
                title="Waste"
                items={losses.wasteAffectedItems}
                reasons={losses.wasteReasons}
              />
              <LossColumn
                title="Stockouts"
                items={losses.stockoutAffectedItems}
                reasons={losses.stockoutReasons}
              />
            </div>
          )}
          {losses.recentWasteQuantities.length > 0 && (
            <BriefBlock title="Recent waste quantity groups">
              <ul className="brief-quantity-list">
                {losses.recentWasteQuantities.map((item) => (
                  <li key={`${item.inventoryItemId}-${item.countUnit}`}>
                    <span>
                      <strong>{item.inventoryItemName}</strong>
                      <small>{item.eventCount} {plural(item.eventCount, "waste log")}</small>
                    </span>
                    <b>{formatDecimal(item.enteredQuantity)} {item.countUnit}</b>
                  </li>
                ))}
              </ul>
            </BriefBlock>
          )}
        </BriefSection>
      </div>
    </>
  );
}

function BriefSection({
  code,
  title,
  caveats,
  children,
}: {
  code: string;
  title: string;
  caveats: string[];
  children: React.ReactNode;
}) {
  return (
    <article className="weekly-brief-section">
      <header>
        <p className="section-code">{code}</p>
        <h2>{title}</h2>
      </header>
      {children}
      <aside className="brief-caveats" aria-label={`${title} limitations`}>
        {caveats.map((caveat) => <p key={caveat}>{caveat}</p>)}
      </aside>
    </article>
  );
}

function BriefStat({ value, label }: { value: string; label: string }) {
  return <div className="brief-stat"><strong>{value}</strong><span>{label}</span></div>;
}

function BriefBlock({ title, children }: { title: string; children: React.ReactNode }) {
  return <section className="brief-block"><h3>{title}</h3>{children}</section>;
}

function CurrencyList({ values }: { values: CurrencyAmount[] }) {
  return (
    <ul className="brief-currency-list">
      {values.map((value) => (
        <li key={value.currency}>
          <span>{value.currency}</span>
          <strong>{formatDecimal(value.amount)}</strong>
        </li>
      ))}
    </ul>
  );
}

function LossColumn({
  title,
  items,
  reasons,
}: {
  title: string;
  items: CountGroup[];
  reasons: ReasonCount[];
}) {
  return (
    <section className="brief-loss-column">
      <h3>{title}</h3>
      <h4>Affected items</h4>
      <CountList values={items.map((item) => ({ label: item.inventoryItemName, count: item.eventCount }))} />
      <h4>Logged reasons</h4>
      <CountList values={reasons.map((reason) => ({ label: reasonLabel(reason.reason), count: reason.eventCount }))} />
    </section>
  );
}

function CountList({ values }: { values: { label: string; count: number }[] }) {
  if (!values.length) return <p className="brief-empty">None logged.</p>;
  return (
    <ul className="brief-count-list">
      {values.map((value) => (
        <li key={value.label}><span>{value.label}</span><strong>{value.count}</strong></li>
      ))}
    </ul>
  );
}

function formatWeekRange(start: string, endExclusive: string) {
  const startDate = parseLocalDate(start);
  const endDate = parseLocalDate(endExclusive);
  endDate.setUTCDate(endDate.getUTCDate() - 1);
  const formatter = new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone: "UTC",
  });
  return `${formatter.format(startDate)} – ${formatter.format(endDate)}`;
}

function parseLocalDate(value: string) {
  const parsed = new Date(`${value}T12:00:00Z`);
  return Number.isNaN(parsed.getTime()) ? new Date(0) : parsed;
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

function formatDecimal(value: string) {
  return value;
}

function reasonLabel(value: string) {
  return value
    .split("_")
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

function plural(count: number, noun: string) {
  return count === 1 ? noun : `${noun}s`;
}
