import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

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

type SalesImportRow = {
  rowNumber: number;
  rawItemLabel: string;
  itemCode: string | null;
  quantity: string;
  reportedNetSales: string | null;
  currency: string | null;
  matchStatus: "matched" | "unmatched";
  matchedMenuItemId: string | null;
  matchedMenuItemName: string | null;
  matchedMenuItemCurrency: string | null;
  validationErrors: string[];
};

type SalesImportPreview = {
  originalFilename: string;
  businessDate: string;
  rows: SalesImportRow[];
  existingDay: SalesDay | null;
};

type ImportDecision = string | "exclude";
type ImportApplyLine = {
  menuItemId: string;
  menuItemName: string;
  quantity: string;
  reportedNetSales: string | null;
  currency: string | null;
  sourceRows: number[];
  effect: "added" | "changed" | "unchanged";
};
type ImportReplacement = {
  lines: ImportApplyLine[];
  removed: SalesLine[];
  errors: string[];
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
  const canWrite = ["owner", "manager", "staff"].includes(restaurant.role);
  const canImport = restaurant.role === "owner" || restaurant.role === "manager";
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
  const [importFile, setImportFile] = useState<File | null>(null);
  const [importPreview, setImportPreview] = useState<SalesImportPreview | null>(null);
  const [importDecisions, setImportDecisions] = useState<Record<number, ImportDecision | undefined>>({});
  const [importing, setImporting] = useState(false);
  const [salesSearch, setSalesSearch] = useState("");
  const [salesCategory, setSalesCategory] = useState("all");
  const [salesView, setSalesView] = useState<"all" | "entered">("all");
  const requestGeneration = useRef(0);

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
      const generation = ++requestGeneration.current;
      setImportPreview(null);
      setImportFile(null);
      setImportDecisions({});
      setLoadingDay(true);
      setError("");
      setNotice("");
      try {
        const value = await request<SalesDay>(`/v1/sales-days/${businessDate}`);
        if (generation !== requestGeneration.current) return;
        adoptDay(value, businessDate);
      } catch (reason) {
        if (generation !== requestGeneration.current) return;
        if (requestStatus(reason) === 404) {
          adoptDay(null, businessDate);
        } else {
          setError(errorMessage(reason, "Sales for this date couldn't load. Try again."));
        }
      } finally {
        if (generation === requestGeneration.current) setLoadingDay(false);
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
  const importReplacement = useMemo(
    () => importPreview ? buildImportReplacement(importPreview, importDecisions, options) : null,
    [importDecisions, importPreview, options],
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

  function updateSalesSearch(value: string) {
    if (!salesSearch.trim() && value.trim()) {
      setSalesCategory("all");
      setSalesView("all");
    }
    setSalesSearch(value);
  }

  function openSelectedDate(event: FormEvent) {
    event.preventDefault();
    if (!dateInput) return;
    void loadDay(dateInput);
  }

  function selectImportFile(file: File | null) {
    requestGeneration.current += 1;
    setImporting(false);
    setImportFile(file);
    setImportPreview(null);
    setImportDecisions({});
    setError("");
    setNotice("");
  }

  async function previewCsv(file = importFile) {
    if (!file) {
      setError("Choose a CSV file to preview.");
      return;
    }
    if (file.size > 1024 * 1024) {
      setError("CSV files must be no larger than 1 MiB.");
      return;
    }
    const generation = ++requestGeneration.current;
    setImporting(true);
    setError("");
    setNotice("");
    try {
      const body = new FormData();
      body.append("file", file);
      const [value, nextOptions] = await Promise.all([
        request<SalesImportPreview>("/v1/sales-imports/preview", {
          method: "POST",
          body,
        }),
        request<MenuOption[]>("/v1/sales/menu-options"),
      ]);
      if (generation !== requestGeneration.current) return;
      setOptions(nextOptions);
      setImportPreview(value);
      setImportDecisions(Object.fromEntries(
        value.rows
          .filter((row) => row.matchedMenuItemId)
          .map((row) => [row.rowNumber, row.matchedMenuItemId as string]),
      ));
      setStale(false);
      window.scrollTo({ top: 0, behavior: "smooth" });
    } catch (reason) {
      if (generation !== requestGeneration.current) return;
      setError(errorMessage(reason, "The CSV couldn't be previewed. Check the file and try again."));
    } finally {
      if (generation === requestGeneration.current) setImporting(false);
    }
  }

  async function applyImport() {
    if (!importPreview || !importReplacement || importReplacement.errors.length > 0) return;
    const generation = ++requestGeneration.current;
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const value = await request<SalesDay>(`/v1/sales-days/${importPreview.businessDate}`, {
        method: "PUT",
        body: JSON.stringify({
          expectedRevision: importPreview.existingDay?.revision ?? null,
          lines: importReplacement.lines.map((line) => ({
            menuItemId: line.menuItemId,
            quantity: line.quantity,
            reportedNetSales: line.reportedNetSales,
            currency: line.currency,
          })),
        }),
      });
      if (generation !== requestGeneration.current) return;
      const replaced = importPreview.existingDay !== null;
      setImportPreview(null);
      setImportFile(null);
      setImportDecisions({});
      adoptDay(value, value.businessDate);
      setNotice(replaced ? "CSV applied. The complete saved day was replaced." : "CSV sales day saved.");
      await loadRecent();
    } catch (reason) {
      if (generation !== requestGeneration.current) return;
      if (requestStatus(reason) === 409) {
        setStale(true);
        setError("Sales for this date changed after the CSV preview. Refresh the preview before applying it.");
      } else {
        setError(errorMessage(reason, "The CSV couldn't be applied. Review the replacement and try again."));
      }
    } finally {
      if (generation === requestGeneration.current) setSaving(false);
    }
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
        <p className="section-code">Daily sales</p>
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
            disabled={importing || saving || importPreview !== null}
            onChange={(event) => setDateInput(event.target.value)}
          />
        </label>
        <button className="file-button" type="submit" disabled={!dateInput || loadingDay || importing || saving || importPreview !== null}>
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
          <p>
            {importPreview
              ? "The CSV is still selected. Refresh its preview against the latest saved day."
              : "Your unsaved entries will be replaced with the latest saved version."}
          </p>
          <button
            className="file-button"
            type="button"
            onClick={() => importPreview && importFile ? void previewCsv(importFile) : void loadDay(loadedDate)}
          >
            {importPreview ? "Refresh CSV preview" : "Reload saved day"}
          </button>
        </div>
      )}

      {canImport && !importPreview && !loading && !dateChanged && (
        <SalesImportUpload
          businessDate={loadedDate}
          file={importFile}
          importing={importing}
          onFile={selectImportFile}
          onPreview={() => void previewCsv()}
        />
      )}

      {!dateChanged && loading ? (
        <p className="sales-loading" role="status">Loading menu items and saved sales…</p>
      ) : !dateChanged && importPreview && importReplacement && canImport ? (
        <SalesImportReview
          preview={importPreview}
          options={options}
          decisions={importDecisions}
          replacement={importReplacement}
          saving={saving}
          stale={stale}
          onDecision={(rowNumber, decision) => {
            setImportDecisions((current) => ({ ...current, [rowNumber]: decision || undefined }));
            if (!stale) setError("");
          }}
          onCancel={() => {
            requestGeneration.current += 1;
            setImporting(false);
            setImportPreview(null);
            setImportDecisions({});
            setError("");
            setStale(false);
          }}
          onApply={() => void applyImport()}
        />
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
          search={salesSearch}
          category={salesCategory}
          view={salesView}
          saving={saving}
          stale={stale}
          onChange={updateEntry}
          onSearch={updateSalesSearch}
          onCategory={setSalesCategory}
          onView={setSalesView}
          onReview={review}
        />
      ) : !dateChanged ? (
        <SalesReadOnly businessDate={loadedDate} day={day} />
      ) : null}

      <RecentSales
        recent={recent}
        currentDate={loadedDate}
        loading={loadingOptions}
        disabled={loadingDay || importing || saving || importPreview !== null}
        onOpen={(businessDate) => void loadDay(businessDate)}
      />
    </section>
  );
}

function SalesImportUpload({
  businessDate,
  file,
  importing,
  onFile,
  onPreview,
}: {
  businessDate: string;
  file: File | null;
  importing: boolean;
  onFile: (file: File | null) => void;
  onPreview: () => void;
}) {
  return (
    <section className="sales-import-upload" aria-labelledby="sales-import-heading">
      <div>
        <p className="section-code">CSV import</p>
        <h2 id="sales-import-heading">Upload sales CSV</h2>
        <p>
          Exact menu names match automatically. You will map or exclude every unmatched row and
          review the complete saved-day replacement before anything changes.
        </p>
      </div>
      <div className="sales-import-controls">
        <input
          className="visually-hidden"
          id="sales-csv-file"
          type="file"
          accept=".csv,text/csv"
          disabled={importing}
          onChange={(event) => onFile(event.target.files?.[0] ?? null)}
        />
        <label
          className={`file-button ${importing ? "file-button-disabled" : ""}`}
          htmlFor="sales-csv-file"
          aria-disabled={importing ? "true" : undefined}
        >
          {file ? "Choose another CSV" : "Choose CSV"}
        </label>
        <button className="ledger-button" type="button" disabled={!file || importing} onClick={onPreview}>
          {importing ? "Checking CSV…" : "Preview CSV"}
        </button>
        <button className="text-button" type="button" onClick={() => downloadSalesCsvTemplate(businessDate)}>
          Download CSV template
        </button>
      </div>
      <p className="selected-file" aria-live="polite">
        {file ? `${file.name} · ${formatFileSize(file.size)}` : "CSV only · 1 MiB maximum · one business date"}
      </p>
    </section>
  );
}

function SalesImportReview({
  preview,
  options,
  decisions,
  replacement,
  saving,
  stale,
  onDecision,
  onCancel,
  onApply,
}: {
  preview: SalesImportPreview;
  options: MenuOption[];
  decisions: Record<number, ImportDecision | undefined>;
  replacement: ImportReplacement;
  saving: boolean;
  stale: boolean;
  onDecision: (rowNumber: number, decision: ImportDecision | "") => void;
  onCancel: () => void;
  onApply: () => void;
}) {
  const attention = preview.rows.filter((row) =>
    row.matchStatus === "unmatched" || row.validationErrors.length > 0,
  );
  const exact = preview.rows.filter((row) =>
    row.matchStatus === "matched" && row.validationErrors.length === 0,
  );
  const excluded = Object.values(decisions).filter((decision) => decision === "exclude").length;
  const unresolved = preview.rows.filter((row) => decisions[row.rowNumber] === undefined).length;
  return (
    <section className="sales-import-review" aria-labelledby="sales-import-review-heading">
      <button className="text-button" type="button" disabled={saving} onClick={onCancel}>
        ← Cancel CSV preview
      </button>
      <p className="section-code">Review CSV import</p>
      <h2 id="sales-import-review-heading">Review {formatBusinessDate(preview.businessDate)}</h2>
      <p className="sales-import-file">
        {preview.originalFilename} · {preview.rows.length} source {preview.rows.length === 1 ? "row" : "rows"}
        {excluded > 0 ? ` · ${excluded} excluded` : ""}
      </p>
      <div className="sales-import-rule" role="note">
        <strong>No fuzzy matching</strong>
        <p>
          Only trimmed, case-insensitive menu names matched automatically. Item codes are shown for
          reference and are not used to match.
        </p>
      </div>

      {attention.length > 0 && (
        <section className="sales-import-source" aria-labelledby="csv-attention-heading">
          <div className="list-heading">
            <h3 id="csv-attention-heading">Needs your decision · {attention.length}</h3>
          </div>
          <div className="sales-import-rows">
            {attention.map((row) => (
              <SalesImportSourceRow
                key={row.rowNumber}
                businessDate={preview.businessDate}
                row={row}
                options={options}
                decision={decisions[row.rowNumber]}
                onDecision={onDecision}
              />
            ))}
          </div>
        </section>
      )}

      {exact.length > 0 && (
        <details className="sales-import-matches">
          <summary>Review {exact.length} exact {exact.length === 1 ? "match" : "matches"}</summary>
          <div className="sales-import-rows">
            {exact.map((row) => (
              <SalesImportSourceRow
                key={row.rowNumber}
                businessDate={preview.businessDate}
                row={row}
                options={options}
                decision={decisions[row.rowNumber]}
                onDecision={onDecision}
              />
            ))}
          </div>
        </details>
      )}

      <section className="sales-import-effect" aria-labelledby="replacement-effect-heading">
        <div className="sales-replacement-heading">
          <div>
            <p className="invoice-status">Complete replacement effect</p>
            <h3 id="replacement-effect-heading">
              {preview.existingDay
                ? `Replace saved revision ${preview.existingDay.revision}`
                : "Create this sales day"}
            </h3>
          </div>
          <p>
            {preview.existingDay?.lines.length ?? 0} currently saved → {replacement.lines.length} after CSV
          </p>
        </div>
        {preview.existingDay && (
          <p className="review-warning">
            Applying replaces the full item list for this date. It does not merge CSV rows into revision {preview.existingDay.revision}.
          </p>
        )}
        {replacement.errors.length > 0 && (
          <div className="sales-import-errors" role="alert">
            <strong>{unresolved > 0 ? "Finish every row before applying" : "The replacement needs attention"}</strong>
            <ul>
              {replacement.errors.map((message) => <li key={message}>{message}</li>)}
            </ul>
          </div>
        )}
        <div className="sales-replacement-lines">
          {replacement.lines.map((line) => (
            <article key={line.menuItemId}>
              <div>
                <p className={`sales-effect sales-effect-${line.effect}`}>{line.effect}</p>
                <h4>{line.menuItemName}</h4>
                <small>CSV {line.sourceRows.map((row) => `row ${row}`).join(", ")}</small>
              </div>
              <p><span>Quantity</span><strong>{line.quantity}</strong></p>
              <p>
                <span>Reported net sales</span>
                <strong>{line.reportedNetSales === null ? "Not reported" : exactMoney(line.reportedNetSales, line.currency)}</strong>
              </p>
            </article>
          ))}
          {replacement.removed.map((line) => (
            <article className="sales-replacement-removed" key={line.menuItemId}>
              <div>
                <p className="sales-effect sales-effect-removed">removed</p>
                <h4>{line.menuItemName}</h4>
                <small>Present in the saved day, not in the included CSV rows</small>
              </div>
              <p><span>Current quantity</span><strong>{trimDecimal(line.quantity)}</strong></p>
              <p>
                <span>Current reported net sales</span>
                <strong>{line.reportedNetSales === null ? "Not reported" : exactMoney(line.reportedNetSales, line.currency)}</strong>
              </p>
            </article>
          ))}
        </div>
      </section>

      <div className="sales-actions">
        <p>
          {replacement.errors.length > 0
            ? "Resolve the listed rows to see an apply-ready replacement."
            : preview.existingDay
              ? `Ready to replace revision ${preview.existingDay.revision} with ${replacement.lines.length} complete lines.`
              : `Ready to create ${replacement.lines.length} complete lines.`}
        </p>
        <button
          className="ledger-button"
          type="button"
          disabled={saving || stale || replacement.errors.length > 0}
          onClick={onApply}
        >
          {saving ? "Applying CSV…" : preview.existingDay ? "Replace complete day" : "Apply CSV day"}
        </button>
      </div>
    </section>
  );
}

function SalesImportSourceRow({
  businessDate,
  row,
  options,
  decision,
  onDecision,
}: {
  businessDate: string;
  row: SalesImportRow;
  options: MenuOption[];
  decision: ImportDecision | undefined;
  onDecision: (rowNumber: number, decision: ImportDecision | "") => void;
}) {
  const selected = options.find((option) => option.id === decision);
  const currencyError = selected && row.currency && selected.currency !== row.currency
    ? `Cannot use ${selected.name}: this row reports ${row.currency}, but the menu item uses ${selected.currency}.`
    : "";
  return (
    <article className={`sales-import-row ${row.matchStatus === "unmatched" ? "sales-import-row-unmatched" : ""}`}>
      <div className="sales-import-row-heading">
        <div>
          <p className="invoice-status">
            CSV row {row.rowNumber} · {row.matchStatus === "matched" ? "Exact name match" : "Unmatched"}
          </p>
          <h4>{row.rawItemLabel}</h4>
          {row.itemCode && <small>Item code: {row.itemCode}</small>}
        </div>
        <span className={`match-status match-status-${row.matchStatus}`}>
          {row.matchStatus === "matched" ? "Matched" : "Needs decision"}
        </span>
      </div>
      <dl className="sales-import-values">
        <div><dt>Business date</dt><dd>{formatBusinessDate(businessDate)}</dd></div>
        <div><dt>Quantity</dt><dd>{row.quantity}</dd></div>
        <div>
          <dt>Reported net sales</dt>
          <dd>{row.reportedNetSales === null ? "Not reported" : exactMoney(row.reportedNetSales, row.currency)}</dd>
        </div>
      </dl>
      <label htmlFor={`sales-import-match-${row.rowNumber}`}>
        Apply this row as
        <select
          id={`sales-import-match-${row.rowNumber}`}
          value={decision ?? ""}
          aria-invalid={currencyError ? "true" : undefined}
          onChange={(event) => onDecision(row.rowNumber, event.target.value as ImportDecision | "")}
        >
          <option value="">Choose a menu item or exclude</option>
          <option value="exclude">Exclude this row from the sales day</option>
          {options.map((option) => (
            <option key={option.id} value={option.id}>{option.name} · {option.currency}</option>
          ))}
        </select>
      </label>
      {decision === "exclude" && <p className="sales-row-resolution">Excluded — this row will not be saved.</p>}
      {selected && !currencyError && (
        <p className="sales-row-resolution">
          {row.matchedMenuItemId === selected.id ? "Exact match" : "Manual match"} — save as {selected.name}.
        </p>
      )}
      {currencyError && <p className="review-warning" role="alert">{currencyError}</p>}
      {!currencyError && decision === row.matchedMenuItemId && row.validationErrors.map((message) => (
        <p className="review-warning" role="alert" key={message}>{message}</p>
      ))}
    </article>
  );
}

function SalesEntry({
  businessDate,
  day,
  rows,
  entries,
  search,
  category,
  view,
  saving,
  stale,
  onChange,
  onSearch,
  onCategory,
  onView,
  onReview,
}: {
  businessDate: string;
  day: SalesDay | null;
  rows: SalesRow[];
  entries: Record<string, Entry>;
  search: string;
  category: string;
  view: "all" | "entered";
  saving: boolean;
  stale: boolean;
  onChange: (id: string, field: keyof Entry, value: string) => void;
  onSearch: (value: string) => void;
  onCategory: (value: string) => void;
  onView: (value: "all" | "entered") => void;
  onReview: () => void;
}) {
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const categories = salesCategoryOptions(rows);
  const filteredRows = rows.filter((row) => {
    const entry = entries[row.id];
    const hasEntry = Boolean(entry?.quantity.trim() || entry?.reportedNetSales.trim());
    return (!normalizedSearch || row.name.toLocaleLowerCase().includes(normalizedSearch))
      && (category === "all" || salesCategoryName(row) === category)
      && (view === "all" || hasEntry);
  });
  const filtersActive = search.trim() !== "" || category !== "all" || view !== "all";
  const groupedRows = groupSalesRows(filteredRows);

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
        <>
          <div className="collection-toolbar sales-item-toolbar" aria-label="Filter sales menu items">
            <label className="collection-search">Search all menu items<input type="search" placeholder="Menu item name" value={search} onChange={(event) => onSearch(event.target.value)}/></label>
            <label>Items<select value={view} onChange={(event) => onView(event.target.value as "all" | "entered")}><option value="all">All menu items</option><option value="entered">With entries</option></select></label>
            <label>Category<select value={category} onChange={(event) => onCategory(event.target.value)}><option value="all">All categories ({rows.length})</option>{categories.map((option) => <option key={option.name} value={option.name}>{option.name} ({option.count})</option>)}</select></label>
            <div className="collection-toolbar-summary"><strong>{filteredRows.length} {filteredRows.length === 1 ? "item" : "items"}</strong>{filtersActive && <button className="text-button" type="button" onClick={() => { onSearch(""); onCategory("all"); onView("all"); }}>Clear filters</button>}</div>
          </div>
          {filteredRows.length === 0 ? <div className="filtered-empty"><h3>{view === "entered" && !search.trim() && category === "all" ? "No entries yet" : "No menu items match"}</h3><p>{view === "entered" && !search.trim() && category === "all" ? "Enter a quantity or reported sales amount, then it will appear in this view." : "Try another name, category, or item view."}</p><button className="file-button" type="button" onClick={() => { onSearch(""); onCategory("all"); onView("all"); }}>Show all menu items</button></div> : <div className="sales-category-groups">{groupedRows.map(([groupName, group]) => <section className="sales-category-group" key={groupName}><h3>{groupName}<span>{group.length}</span></h3><div className="sales-rows">{group.map((row) => {
              const entry = entries[row.id] ?? { quantity: "", reportedNetSales: "" };
              return <article className="sales-row" key={row.id}><div className="sales-row-name"><p className="invoice-status">{row.inactive ? "Saved item · no longer active" : "Menu item"}</p><h4>{row.name}</h4></div><label>Quantity sold<input aria-label={`${row.name}, quantity sold`} inputMode="decimal" placeholder="0" value={entry.quantity} onChange={(event) => onChange(row.id, "quantity", event.target.value)}/></label><label>Reported net sales <span>Optional{row.currency ? ` · ${row.currency}` : ""}</span><input aria-label={`${row.name}, optional reported net sales`} inputMode="decimal" placeholder="Leave blank" value={entry.reportedNetSales} onChange={(event) => onChange(row.id, "reportedNetSales", event.target.value)}/></label></article>;
            })}</div></section>)}</div>}
        </>
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
      <p className="section-code">Daily sales review</p>
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
        <p>Team members can record or correct a complete day.</p>
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
  disabled,
  onOpen,
}: {
  recent: SalesDaySummary[];
  currentDate: string;
  loading: boolean;
  disabled: boolean;
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
                disabled={disabled}
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

function salesCategoryName(row: SalesRow) {
  return row.inactive ? "Saved inactive items" : row.category?.trim() || "Uncategorized";
}

function salesCategoryOptions(rows: SalesRow[]) {
  const counts = new Map<string, number>();
  rows.forEach((row) => {
    const name = salesCategoryName(row);
    counts.set(name, (counts.get(name) ?? 0) + 1);
  });
  return [...counts].map(([name, count]) => ({ name, count })).sort((a, b) => a.name.localeCompare(b.name));
}

function groupSalesRows(rows: SalesRow[]): [string, SalesRow[]][] {
  const groups = new Map<string, SalesRow[]>();
  rows.forEach((row) => {
    const name = salesCategoryName(row);
    groups.set(name, [...(groups.get(name) ?? []), row]);
  });
  return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
}

function buildImportReplacement(
  preview: SalesImportPreview,
  decisions: Record<number, ImportDecision | undefined>,
  options: MenuOption[],
): ImportReplacement {
  type Group = {
    option: MenuOption;
    quantities: string[];
    netSales: string[];
    sourceRows: number[];
  };
  const errors: string[] = [];
  const optionsById = new Map(options.map((option) => [option.id, option]));
  const groups = new Map<string, Group>();

  for (const row of preview.rows) {
    const decision = decisions[row.rowNumber];
    if (decision === undefined) {
      errors.push(`CSV row ${row.rowNumber} (${row.rawItemLabel}) must be mapped or explicitly excluded.`);
      continue;
    }
    if (decision === "exclude") continue;
    const option = optionsById.get(decision);
    if (!option) {
      errors.push(`CSV row ${row.rowNumber} no longer has a valid active menu item.`);
      continue;
    }
    if (row.reportedNetSales !== null && row.currency !== option.currency) {
      errors.push(
        `CSV row ${row.rowNumber} reports ${row.currency ?? "no currency"}; ${option.name} uses ${option.currency}.`,
      );
      continue;
    }
    const group = groups.get(option.id) ?? {
      option,
      quantities: [],
      netSales: [],
      sourceRows: [],
    };
    group.quantities.push(row.quantity);
    if (row.reportedNetSales !== null) group.netSales.push(row.reportedNetSales);
    group.sourceRows.push(row.rowNumber);
    groups.set(option.id, group);
  }

  if (groups.size === 0) errors.push("Include at least one CSV row in the replacement.");
  if (groups.size > 200) errors.push("The replacement may contain no more than 200 menu items.");

  const currentById = new Map(
    (preview.existingDay?.lines ?? []).map((line) => [line.menuItemId, line]),
  );
  const lines = [...groups.values()]
    .map((group): ImportApplyLine => {
      const quantity = sumExactDecimals(group.quantities, 6);
      const reportedNetSales = group.netSales.length > 0
        ? sumExactDecimals(group.netSales, 4)
        : null;
      if (!isDecimal(quantity, 6, false)) {
        errors.push(
          `CSV rows ${group.sourceRows.join(", ")} total to a quantity too large to save for ${group.option.name}.`,
        );
      }
      if (reportedNetSales !== null && !isDecimal(reportedNetSales, 4, true)) {
        errors.push(
          `CSV rows ${group.sourceRows.join(", ")} total to net sales too large to save for ${group.option.name}.`,
        );
      }
      const currency = reportedNetSales === null ? null : group.option.currency;
      const current = currentById.get(group.option.id);
      const unchanged = current !== undefined
        && exactDecimalEqual(current.quantity, quantity, 6)
        && nullableDecimalEqual(current.reportedNetSales, reportedNetSales, 4)
        && current.currency === currency;
      return {
        menuItemId: group.option.id,
        menuItemName: group.option.name,
        quantity,
        reportedNetSales,
        currency,
        sourceRows: group.sourceRows,
        effect: current ? (unchanged ? "unchanged" : "changed") : "added",
      };
    })
    .sort((left, right) => left.menuItemName.localeCompare(right.menuItemName));
  const includedIds = new Set(lines.map((line) => line.menuItemId));
  const removed = (preview.existingDay?.lines ?? [])
    .filter((line) => !includedIds.has(line.menuItemId))
    .sort((left, right) => left.menuItemName.localeCompare(right.menuItemName));
  return { lines, removed, errors };
}

function sumExactDecimals(values: string[], scale: number): string {
  const factor = 10n ** BigInt(scale);
  const total = values.reduce((sum, value) => {
    const [integer, fraction = ""] = value.split(".");
    const scaledFraction = `${fraction}${"0".repeat(scale)}`.slice(0, scale);
    return sum + BigInt(integer) * factor + BigInt(scaledFraction || "0");
  }, 0n);
  const integer = total / factor;
  const fraction = (total % factor).toString().padStart(scale, "0").replace(/0+$/, "");
  return fraction ? `${integer}.${fraction}` : integer.toString();
}

function exactDecimalEqual(left: string, right: string, scale: number): boolean {
  return sumExactDecimals([left], scale) === sumExactDecimals([right], scale);
}

function nullableDecimalEqual(
  left: string | null,
  right: string | null,
  scale: number,
): boolean {
  return left === null || right === null
    ? left === right
    : exactDecimalEqual(left, right, scale);
}

function downloadSalesCsvTemplate(businessDate: string) {
  const csv = [
    "business_date,item_name,quantity,item_code,net_sales,currency",
    `${businessDate},Chicken Taco,84,TACO-CHICKEN,1008.00,USD`,
    `${businessDate},Chips and Salsa,31,,,`,
  ].join("\n");
  const url = URL.createObjectURL(new Blob([csv], { type: "text/csv;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = `parline-sales-${businessDate}.csv`;
  link.click();
  URL.revokeObjectURL(url);
}

function formatFileSize(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KiB`;
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
