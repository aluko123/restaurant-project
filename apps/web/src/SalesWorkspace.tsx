import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";

export type ApiRequest = <T>(path: string, init?: RequestInit) => Promise<T>;

type Restaurant = {
  name: string;
  role: string;
  timezone: string;
};

type MenuOption = {
  id: string;
  name: string;
  category: string | null;
  currency: string;
};

type SalesLine = {
  menuItemId: string;
  menuItemName: string;
  quantity: string;
  reportedNetSales: string | null;
  currency: string | null;
};

type SalesDay = {
  id: string;
  businessDate: string;
  revision: number;
  createdAt: string;
  updatedAt: string;
  lines: SalesLine[];
};

type SalesDaySummary = {
  businessDate: string;
  revision: number;
  lineCount: number;
  totalQuantity: string;
  reportedLineCount: number;
  updatedAt: string;
};

type Entry = { quantity: string; reportedNetSales: string };
type SalesRow = {
  id: string;
  name: string;
  category: string | null;
  currency: string | null;
  inactive: boolean;
};

export function SalesWorkspace({
  restaurant,
  request,
  active,
}: {
  restaurant: Restaurant;
  request: ApiRequest;
  active: boolean;
}) {
  const canWrite = restaurant.role === "owner" || restaurant.role === "manager";
  const today = useMemo(() => businessDateInZone(restaurant.timezone), [restaurant.timezone]);
  const defaultDate = useMemo(() => previousBusinessDate(today), [today]);
  const [dateInput, setDateInput] = useState(defaultDate);
  const [loadedDate, setLoadedDate] = useState(defaultDate);
  const [options, setOptions] = useState<MenuOption[]>([]);
  const [recent, setRecent] = useState<SalesDaySummary[]>([]);
  const [day, setDay] = useState<SalesDay | null>(null);
  const [entries, setEntries] = useState<Record<string, Entry>>({});
  const [mode, setMode] = useState<"entry" | "review">("entry");
  const [loadingOptions, setLoadingOptions] = useState(true);
  const [loadingDay, setLoadingDay] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [stale, setStale] = useState(false);

  const adoptDay = useCallback((value: SalesDay | null, businessDate: string) => {
    setDay(value);
    setLoadedDate(businessDate);
    setDateInput(businessDate);
    setEntries(
      Object.fromEntries(
        (value?.lines ?? []).map((line) => [
          line.menuItemId,
          {
            quantity: trimDecimal(line.quantity),
            reportedNetSales: line.reportedNetSales === null ? "" : trimDecimal(line.reportedNetSales),
          },
        ]),
      ),
    );
    setMode("entry");
    setStale(false);
  }, []);

  const loadDay = useCallback(
    async (businessDate: string) => {
      setLoadingDay(true);
      setError("");
      setNotice("");
      try {
        const value = await request<SalesDay>(`/v1/sales-days/${businessDate}`);
        adoptDay(value, businessDate);
      } catch (reason) {
        if (requestStatus(reason) === 404) {
          adoptDay(null, businessDate);
        } else {
          setError(errorMessage(reason, "Sales for this date couldn't load. Try again."));
        }
      } finally {
        setLoadingDay(false);
      }
    },
    [adoptDay, request],
  );

  const loadRecent = useCallback(async () => {
    try {
      setRecent(await request<SalesDaySummary[]>("/v1/sales-days"));
    } catch (reason) {
      setError(errorMessage(reason, "Recent sales days couldn't load. Try again."));
    }
  }, [request]);

  useEffect(() => {
    if (!active) return;
    setLoadingOptions(true);
    setError("");
    void Promise.all([
      request<MenuOption[]>("/v1/sales/menu-options").then(setOptions),
      loadRecent(),
      loadDay(defaultDate),
    ])
      .catch((reason: unknown) => {
        setError(errorMessage(reason, "The sales workspace couldn't load. Try again."));
      })
      .finally(() => setLoadingOptions(false));
  }, [active, defaultDate, loadDay, loadRecent, request]);

  const rows = useMemo(() => salesRows(options, day), [day, options]);
  const chosen = useMemo(
    () =>
      rows
        .filter((row) => entries[row.id]?.quantity.trim())
        .map((row) => ({ row, entry: entries[row.id] })),
    [entries, rows],
  );

  function updateEntry(id: string, field: keyof Entry, value: string) {
    setEntries((current) => ({
      ...current,
      [id]: {
        quantity: current[id]?.quantity ?? "",
        reportedNetSales: current[id]?.reportedNetSales ?? "",
        [field]: value,
      },
    }));
    setNotice("");
  }

  function openSelectedDate(event: FormEvent) {
    event.preventDefault();
    if (!dateInput) return;
    void loadDay(dateInput);
  }

  function review() {
    setError("");
    setNotice("");
    const orphanedSales = rows.some(
      (row) =>
        entries[row.id]?.reportedNetSales.trim() && !entries[row.id]?.quantity.trim(),
    );
    if (orphanedSales) {
      setError("Add a quantity for every item with reported net sales, or clear that sales amount.");
      return;
    }
    if (chosen.length === 0) {
      setError("Add a quantity for at least one menu item.");
      return;
    }
    if (chosen.length > 200) {
      setError("Choose no more than 200 menu items for one sales day.");
      return;
    }
    if (chosen.some(({ entry }) => !isDecimal(entry.quantity, 6, false))) {
      setError("Quantities must be positive plain decimals with no more than 6 decimal places.");
      return;
    }
    if (
      chosen.some(
        ({ entry }) =>
          entry.reportedNetSales.trim() && !isDecimal(entry.reportedNetSales, 4, true),
      )
    ) {
      setError("Reported net sales must be nonnegative plain decimals with no more than 4 decimal places.");
      return;
    }
    setMode("review");
    window.scrollTo({ top: 0, behavior: "smooth" });
  }

  async function save() {
    setSaving(true);
    setError("");
    setNotice("");
    setStale(false);
    try {
      const value = await request<SalesDay>(`/v1/sales-days/${loadedDate}`, {
        method: "PUT",
        body: JSON.stringify({
          expectedRevision: day?.revision ?? null,
          lines: chosen.map(({ row, entry }) => ({
            menuItemId: row.id,
            quantity: entry.quantity.trim(),
            reportedNetSales: entry.reportedNetSales.trim() || null,
          })),
        }),
      });
      const corrected = day !== null;
      adoptDay(value, loadedDate);
      setNotice(corrected ? "Sales day corrected." : "Sales day saved.");
      await loadRecent();
    } catch (reason) {
      if (requestStatus(reason) === 409) {
        setStale(true);
        setMode("entry");
        setError("This sales day changed since you opened it. Reload the saved day before editing again.");
      } else {
        setError(errorMessage(reason, "The sales day couldn't be saved. Check the entries and try again."));
      }
    } finally {
      setSaving(false);
    }
  }

  const dateChanged = dateInput !== loadedDate;
  const loading = loadingOptions || loadingDay;

  return (
    <section className="sales-workspace" aria-labelledby="sales-heading">
      <header className="sales-heading">
        <p className="section-code">DB—DAILY SALES</p>
        <h1 id="sales-heading">Record the day</h1>
        <p>
          Enter item quantities for one complete business day. Reported net sales are optional and
          are never estimated from menu prices.
        </p>
      </header>

      <form className="sales-date-bar" onSubmit={openSelectedDate}>
        <label htmlFor="sales-business-date">
          Business date
          <input
            id="sales-business-date"
            type="date"
            max={today}
            value={dateInput}
            onChange={(event) => setDateInput(event.target.value)}
          />
        </label>
        <button className="file-button" type="submit" disabled={!dateInput || loadingDay}>
          {loadingDay ? "Opening…" : "Open day"}
        </button>
        <small>Defaults to the previous day in {restaurant.timezone}.</small>
      </form>

      {dateChanged && (
        <p className="sales-date-prompt" role="status">
          Open {formatBusinessDate(dateInput)} to view or edit that day.
        </p>
      )}
      {error && <p className="form-error sales-message" role="alert">{error}</p>}
      {notice && <p className="success-notice sales-message" role="status">{notice}</p>}
      {stale && (
        <div className="stale-sales" role="alert">
          <strong>Reload before saving</strong>
          <p>Your unsaved entries will be replaced with the latest saved version.</p>
          <button className="file-button" type="button" onClick={() => void loadDay(loadedDate)}>
            Reload saved day
          </button>
        </div>
      )}

      {!dateChanged && loading ? (
        <p className="sales-loading" role="status">Loading menu items and saved sales…</p>
      ) : !dateChanged && mode === "review" && canWrite ? (
        <SalesReview
          businessDate={loadedDate}
          chosen={chosen}
          saving={saving}
          onBack={() => setMode("entry")}
          onSave={() => void save()}
        />
      ) : !dateChanged && canWrite ? (
        <SalesEntry
          businessDate={loadedDate}
          day={day}
          entries={entries}
          rows={rows}
          saving={saving}
          stale={stale}
          onChange={updateEntry}
          onReview={review}
        />
      ) : !dateChanged ? (
        <SalesReadOnly businessDate={loadedDate} day={day} />
      ) : null}

      <RecentSales
        recent={recent}
        currentDate={loadedDate}
        loading={loadingOptions}
        onOpen={(businessDate) => void loadDay(businessDate)}
      />
    </section>
  );
}

function SalesEntry({
  businessDate,
  day,
  rows,
  entries,
  saving,
  stale,
  onChange,
  onReview,
}: {
  businessDate: string;
  day: SalesDay | null;
  rows: SalesRow[];
  entries: Record<string, Entry>;
  saving: boolean;
  stale: boolean;
  onChange: (id: string, field: keyof Entry, value: string) => void;
  onReview: () => void;
}) {
  return (
    <section className="sales-entry" aria-labelledby="sales-entry-heading">
      <div className="sales-day-title">
        <div>
          <p className="invoice-status">{day ? `Saved · revision ${day.revision}` : "Not saved yet"}</p>
          <h2 id="sales-entry-heading">{formatBusinessDate(businessDate)}</h2>
        </div>
        <p>Quantity is required for selected items. Reported net sales are optional.</p>
      </div>
      {rows.length === 0 ? (
        <p className="empty-state">
          No active menu items are ready. Add the restaurant's first active item in Menu, then
          return here.
        </p>
      ) : (
        <div className="sales-rows">
          {rows.map((row) => {
            const entry = entries[row.id] ?? { quantity: "", reportedNetSales: "" };
            return (
              <article className="sales-row" key={row.id}>
                <div className="sales-row-name">
                  <p className="invoice-status">
                    {row.inactive ? "Saved item · no longer active" : row.category ?? "Uncategorized"}
                  </p>
                  <h3>{row.name}</h3>
                </div>
                <label>
                  Quantity sold
                  <input
                    aria-label={`${row.name}, quantity sold`}
                    inputMode="decimal"
                    placeholder="0"
                    value={entry.quantity}
                    onChange={(event) => onChange(row.id, "quantity", event.target.value)}
                  />
                </label>
                <label>
                  Reported net sales <span>Optional{row.currency ? ` · ${row.currency}` : ""}</span>
                  <input
                    aria-label={`${row.name}, optional reported net sales`}
                    inputMode="decimal"
                    placeholder="Leave blank"
                    value={entry.reportedNetSales}
                    onChange={(event) => onChange(row.id, "reportedNetSales", event.target.value)}
                  />
                </label>
              </article>
            );
          })}
        </div>
      )}
      {rows.length > 0 && (
        <div className="sales-actions">
          <p>{day ? "Saving replaces this day's full item list." : "Nothing is saved until review."}</p>
          <button className="ledger-button" type="button" disabled={saving || stale} onClick={onReview}>
            Review day
          </button>
        </div>
      )}
    </section>
  );
}

function SalesReview({
  businessDate,
  chosen,
  saving,
  onBack,
  onSave,
}: {
  businessDate: string;
  chosen: { row: SalesRow; entry: Entry }[];
  saving: boolean;
  onBack: () => void;
  onSave: () => void;
}) {
  const reported = chosen.filter(({ entry }) => entry.reportedNetSales.trim()).length;
  return (
    <section className="sales-review" aria-labelledby="sales-review-heading">
      <button className="text-button" type="button" disabled={saving} onClick={onBack}>
        ← Back to quantities
      </button>
      <p className="section-code">DB—DAILY SALES / REVIEW</p>
      <h2 id="sales-review-heading">Review {formatBusinessDate(businessDate)}</h2>
      <p>
        {chosen.length} {chosen.length === 1 ? "item" : "items"} selected · {reported} with reported
        net sales
      </p>
      <div className="sales-review-lines">
        {chosen.map(({ row, entry }) => (
          <article key={row.id}>
            <div>
              <p className="invoice-status">Quantity sold</p>
              <h3>{row.name}</h3>
            </div>
            <strong>{entry.quantity.trim()}</strong>
            <p>
              <span>Reported net sales</span>
              {entry.reportedNetSales.trim()
                ? exactMoney(entry.reportedNetSales.trim(), row.currency)
                : "Not reported"}
            </p>
          </article>
        ))}
      </div>
      <div className="sales-actions">
        <button className="file-button" type="button" disabled={saving} onClick={onBack}>
          Edit quantities
        </button>
        <button className="ledger-button" type="button" disabled={saving} onClick={onSave}>
          {saving ? "Saving…" : "Save complete day"}
        </button>
      </div>
    </section>
  );
}

function SalesReadOnly({ businessDate, day }: { businessDate: string; day: SalesDay | null }) {
  return (
    <section className="sales-read-only" aria-labelledby="saved-sales-heading">
      <div className="sales-day-title">
        <div>
          <p className="invoice-status">Saved sales · read only</p>
          <h2 id="saved-sales-heading">{formatBusinessDate(businessDate)}</h2>
        </div>
        <p>Owners and managers can record or correct a complete day.</p>
      </div>
      {!day ? (
        <p className="empty-state">No sales are saved for this business date.</p>
      ) : (
        <div className="sales-review-lines">
          {day.lines.map((line) => (
            <article key={line.menuItemId}>
              <div>
                <p className="invoice-status">Quantity sold</p>
                <h3>{line.menuItemName}</h3>
              </div>
              <strong>{trimDecimal(line.quantity)}</strong>
              <p>
                <span>Reported net sales</span>
                {line.reportedNetSales === null
                  ? "Not reported"
                  : exactMoney(trimDecimal(line.reportedNetSales), line.currency)}
              </p>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function RecentSales({
  recent,
  currentDate,
  loading,
  onOpen,
}: {
  recent: SalesDaySummary[];
  currentDate: string;
  loading: boolean;
  onOpen: (businessDate: string) => void;
}) {
  return (
    <section className="recent-sales" aria-labelledby="recent-sales-heading">
      <div className="list-heading">
        <h2 id="recent-sales-heading">Recent saved days</h2>
      </div>
      {loading ? (
        <p role="status">Loading recent days…</p>
      ) : recent.length === 0 ? (
        <p className="empty-state">No sales days yet. Record the first complete business day above.</p>
      ) : (
        <div className="recent-sales-list">
          {recent.map((summary) => (
            <article key={summary.businessDate}>
              <div>
                <p className="invoice-status">
                  {summary.lineCount} {summary.lineCount === 1 ? "item" : "items"} · revision {summary.revision}
                </p>
                <h3>{formatBusinessDate(summary.businessDate)}</h3>
                <p>
                  {trimDecimal(summary.totalQuantity)} total quantity · {summary.reportedLineCount} with
                  reported sales
                </p>
              </div>
              <button
                className="file-button"
                type="button"
                aria-current={summary.businessDate === currentDate ? "true" : undefined}
                onClick={() => onOpen(summary.businessDate)}
              >
                {summary.businessDate === currentDate ? "Reload day" : "Open day"}
              </button>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function salesRows(options: MenuOption[], day: SalesDay | null): SalesRow[] {
  const active = options.map((option) => ({
    id: option.id,
    name: option.name,
    category: option.category,
    currency: option.currency,
    inactive: false,
  }));
  const activeIds = new Set(options.map((option) => option.id));
  const inactive = (day?.lines ?? [])
    .filter((line) => !activeIds.has(line.menuItemId))
    .map((line) => ({
      id: line.menuItemId,
      name: line.menuItemName,
      category: null,
      currency: line.currency,
      inactive: true,
    }));
  return [...active, ...inactive];
}

function businessDateInZone(timeZone: string): string {
  try {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).formatToParts(new Date());
    const part = (type: Intl.DateTimeFormatPartTypes) =>
      parts.find((value) => value.type === type)?.value ?? "";
    const date = `${part("year")}-${part("month")}-${part("day")}`;
    if (/^\d{4}-\d{2}-\d{2}$/.test(date)) return date;
  } catch {
    // Existing restaurants may predate timezone validation; UTC is the conservative fallback.
  }
  return new Date().toISOString().slice(0, 10);
}

function previousBusinessDate(value: string): string {
  const [year, month, day] = value.split("-").map(Number);
  const date = new Date(Date.UTC(year, month - 1, day));
  date.setUTCDate(date.getUTCDate() - 1);
  return date.toISOString().slice(0, 10);
}

function isDecimal(value: string, scale: number, allowZero: boolean): boolean {
  const match = /^(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!match || (match[2]?.length ?? 0) > scale) return false;
  if (!allowZero && /^0+(?:\.0+)?$/.test(value.trim())) return false;
  const integerDigits = match[1].replace(/^0+/, "").length || 1;
  return integerDigits <= 18 - scale;
}

function requestStatus(reason: unknown): number | undefined {
  if (reason instanceof Error && "status" in reason) {
    const status = (reason as Error & { status?: unknown }).status;
    return typeof status === "number" ? status : undefined;
  }
  return undefined;
}

function errorMessage(reason: unknown, fallback: string): string {
  return reason instanceof Error ? reason.message : fallback;
}

function formatBusinessDate(value: string): string {
  const date = new Date(`${value}T00:00:00Z`);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeZone: "UTC" }).format(date);
}

function trimDecimal(value: string): string {
  return value.replace(/\.0+$/, "").replace(/(\.\d*?)0+$/, "$1");
}

function exactMoney(value: string, currency: string | null): string {
  return `${currency ?? "Reported"} ${trimDecimal(value)}`;
}
