import { useEffect, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";

export type SupplierOption = { id: string; name: string };

export type OrderGuideLine = {
  id: string;
  inventoryItemId: string;
  inventoryItemName: string;
  countUnit: string;
  countedQuantity: string;
  parLevel: string;
  shortage: string;
  supplierId: string | null;
  supplierName: string | null;
  orderUnit: string;
  conversion: string;
  suggestedOrderQuantity: string;
  orderQuantity: string;
  receivedQuantity: string | null;
  receiptStatus: string | null;
  discrepancyKind: string | null;
};

export type OrderGuide = {
  id: string;
  sourceCountId: string;
  status: "draft" | "ordered" | "received" | "cancelled";
  revision: number;
  createdAt: string;
  updatedAt: string;
  orderedAt: string | null;
  receivedAt: string | null;
  cancelledAt: string | null;
  linkedInvoiceId: string | null;
  linkedInvoiceSupplierName: string | null;
  linkedInvoiceDate: string | null;
  lines: OrderGuideLine[];
};

type LinkableInvoice = {
  id: string;
  supplierName: string;
  invoiceDate: string;
  status: string;
};

function formatQuantity(value: string) {
  const [whole, fraction] = value.split(".");
  if (fraction === undefined) return value;
  const trimmed = fraction.replace(/0+$/, "");
  return trimmed ? `${whole}.${trimmed}` : whole;
}

function displayGuide(guide: OrderGuide): OrderGuide {
  return {
    ...guide,
    lines: guide.lines.map((line) => ({
      ...line,
      countedQuantity: formatQuantity(line.countedQuantity),
      parLevel: formatQuantity(line.parLevel),
      shortage: formatQuantity(line.shortage),
      conversion: formatQuantity(line.conversion),
      suggestedOrderQuantity: formatQuantity(line.suggestedOrderQuantity),
      orderQuantity: formatQuantity(line.orderQuantity),
      receivedQuantity:
        line.receivedQuantity === null ? null : formatQuantity(line.receivedQuantity),
    })),
  };
}

function discrepancyCopy(line: OrderGuideLine): string | null {
  const kind = line.discrepancyKind;
  if (!kind || kind === "none") return null;
  if (kind === "missing") return "Missing — nothing arrived for this line.";
  if (kind === "short") {
    return `Short — ordered ${line.orderQuantity}, got ${line.receivedQuantity}.`;
  }
  if (kind === "over") {
    return `Extra — ordered ${line.orderQuantity}, got ${line.receivedQuantity}.`;
  }
  return null;
}

export function OrderGuidePanel({
  guide,
  manager,
  request,
  suppliers = [],
  onChange,
}: {
  guide: OrderGuide;
  manager: boolean;
  request: ApiRequest;
  suppliers?: SupplierOption[];
  onChange: (guide: OrderGuide | null, notice?: string) => void;
}) {
  const [draft, setDraft] = useState(() => displayGuide(guide));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [dirty, setDirty] = useState(false);
  const [receive, setReceive] = useState<Record<string, { selected: boolean; quantity: string }>>(
    {},
  );
  const [invoices, setInvoices] = useState<LinkableInvoice[]>([]);
  const [linkedInvoiceId, setLinkedInvoiceId] = useState("");

  useEffect(() => {
    const displayed = displayGuide(guide);
    setDraft(displayed);
    setDirty(false);
    setLinkedInvoiceId(displayed.linkedInvoiceId ?? "");
    setReceive(
      Object.fromEntries(
        displayed.lines
          .filter((x) => x.receivedQuantity === null)
          .map((x) => [x.id, { selected: false, quantity: x.orderQuantity }]),
      ),
    );
  }, [guide]);

  useEffect(() => {
    if (guide.status !== "ordered") return;
    let cancelled = false;
    void request<LinkableInvoice[]>("/v1/invoices")
      .then((rows) => {
        if (!cancelled) {
          setInvoices(rows.filter((row) => row.status === "ready").slice(0, 30));
        }
      })
      .catch(() => {
        if (!cancelled) setInvoices([]);
      });
    return () => {
      cancelled = true;
    };
  }, [guide.status, guide.id, request]);

  const edit = (id: string, patch: Partial<OrderGuideLine>) => {
    setDraft((x) => ({
      ...x,
      lines: x.lines.map((line) => (line.id === id ? { ...line, ...patch } : line)),
    }));
    setDirty(true);
  };

  async function save() {
    const next = await request<OrderGuide>(`/v1/order-guides/${draft.id}`, {
      method: "PUT",
      body: JSON.stringify({
        revision: draft.revision,
        lines: draft.lines.map(
          ({ id, supplierId, supplierName, orderUnit, conversion, orderQuantity }) => ({
            id,
            supplierId: supplierId || null,
            supplierName: supplierId ? null : supplierName?.trim() || null,
            orderUnit,
            conversion,
            orderQuantity,
          }),
        ),
      }),
    });
    setDraft(displayGuide(next));
    setDirty(false);
    onChange(next);
    return next;
  }

  async function act(action: "save" | "ordered" | "cancel") {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      if (action === "save") {
        await save();
        setNotice("Order guide saved.");
        return;
      }
      if (action === "cancel") {
        if (!window.confirm("Cancel this order guide? This cannot be undone.")) return;
        await request(`/v1/order-guides/${draft.id}/cancel`, {
          method: "POST",
          body: JSON.stringify({ revision: draft.revision }),
        });
        onChange(null, "Order guide cancelled.");
        return;
      }
      const current = dirty ? await save() : draft;
      const next = await request<OrderGuide>(`/v1/order-guides/${draft.id}/ordered`, {
        method: "POST",
        body: JSON.stringify({ revision: current.revision }),
      });
      onChange(
        next,
        "Order recorded. Inventory quantities remain based on the last physical count.",
      );
    } catch (reason) {
      setError(
        reason instanceof Error
          ? reason.message
          : "The order guide couldn't be updated. Try again.",
      );
    } finally {
      setBusy(false);
    }
  }

  async function submitReceipt() {
    const lines = Object.entries(receive)
      .filter(([, x]) => x.selected)
      .map(([id, x]) => ({ id, receivedQuantity: x.quantity.trim() }));
    if (!lines.length) {
      setError("Select at least one line to receive.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const next = await request<OrderGuide>(`/v1/order-guides/${draft.id}/receive`, {
        method: "POST",
        body: JSON.stringify({
          lines,
          linkedInvoiceId: linkedInvoiceId || null,
        }),
      });
      const mismatches = next.lines.filter(
        (line) => line.discrepancyKind && line.discrepancyKind !== "none",
      ).length;
      if (next.status === "received") {
        onChange(
          null,
          mismatches
            ? `Receipt recorded with ${mismatches} line${mismatches === 1 ? "" : "s"} that didn't match. Complete another physical count before creating a new guide.`
            : "Receipt recorded. Complete another physical count before creating a new guide.",
        );
      } else {
        onChange(
          next,
          mismatches
            ? `Selected receipt lines recorded. ${mismatches} didn't match what you ordered.`
            : "Selected receipt lines recorded.",
        );
      }
    } catch (reason) {
      setError(
        reason instanceof Error
          ? reason.message
          : "The receipt couldn't be recorded. Check quantities and try again.",
      );
    } finally {
      setBusy(false);
    }
  }

  const ordered = draft.status === "ordered";
  const unresolved = draft.lines.filter((x) => x.receivedQuantity === null);
  const receivedMismatches = draft.lines.filter(
    (line) => line.discrepancyKind && line.discrepancyKind !== "none",
  );

  return (
    <section className="order-guide-panel" aria-labelledby="order-guide-heading">
      <p className="section-code">Next purchasing task</p>
      <h2 id="order-guide-heading">{ordered ? "Receive order" : "Review order guide"}</h2>
      <p>
        {ordered
          ? "This records an order placed outside Parline and does not change inventory. Select only the lines received now; enter 0 when an item is missing."
          : "Suggestions compare your latest completed physical count with each item's par level. Only items counted below par appear here."}
      </p>
      {!manager && !ordered && (
        <p className="review-warning">
          A manager must review, edit, order, or cancel this draft.
        </p>
      )}
      {receivedMismatches.length > 0 && (
        <p className="review-warning" role="status">
          Some lines didn't match what you ordered. Check the delivery before you close this out.
        </p>
      )}
      {draft.linkedInvoiceSupplierName && (
        <p className="success-notice" role="status">
          Invoice linked: {draft.linkedInvoiceSupplierName}
          {draft.linkedInvoiceDate ? ` · ${draft.linkedInvoiceDate}` : ""}
        </p>
      )}
      {draft.lines.map((line) => {
        const mismatch = discrepancyCopy(line);
        return (
          <article className="guide-line" key={line.id}>
            <div>
              <h3>{line.inventoryItemName}</h3>
              <dl className="guide-evidence">
                <div>
                  <dt>Last counted</dt>
                  <dd>
                    {line.countedQuantity} {line.countUnit}
                  </dd>
                </div>
                <div>
                  <dt>Par level</dt>
                  <dd>
                    {line.parLevel} {line.countUnit}
                  </dd>
                </div>
                <div>
                  <dt>Below par by</dt>
                  <dd>
                    {line.shortage} {line.countUnit}
                  </dd>
                </div>
                <div>
                  <dt>Suggested order</dt>
                  <dd>
                    {line.suggestedOrderQuantity} {line.orderUnit}
                  </dd>
                </div>
              </dl>
            </div>
            {!ordered && manager ? (
              <div className="inventory-form-fields">
                <label>
                  Supplier
                  <select
                    value={line.supplierId ?? ""}
                    onChange={(e) => {
                      const id = e.target.value;
                      const match = suppliers.find((s) => s.id === id);
                      edit(line.id, {
                        supplierId: id || null,
                        supplierName: match?.name ?? null,
                      });
                    }}
                  >
                    <option value="">Supplier not set</option>
                    {suppliers.map((supplier) => (
                      <option key={supplier.id} value={supplier.id}>
                        {supplier.name}
                      </option>
                    ))}
                    {line.supplierId &&
                      !suppliers.some((s) => s.id === line.supplierId) &&
                      line.supplierName && (
                        <option value={line.supplierId}>{line.supplierName}</option>
                      )}
                  </select>
                </label>
                <label>
                  Order in
                  <input
                    value={line.orderUnit}
                    maxLength={40}
                    onChange={(e) => edit(line.id, { orderUnit: e.target.value })}
                  />
                </label>
                <label>
                  Count units in one order unit <span>Measured in {line.countUnit}</span>
                  <input
                    inputMode="decimal"
                    value={line.conversion}
                    onChange={(e) => edit(line.id, { conversion: e.target.value })}
                  />
                </label>
                <label>
                  Quantity to order
                  <input
                    inputMode="decimal"
                    value={line.orderQuantity}
                    onChange={(e) => edit(line.id, { orderQuantity: e.target.value })}
                  />
                </label>
              </div>
            ) : ordered && line.receivedQuantity === null ? (
              <div className="receive-fields">
                <label className="active-toggle">
                  <input
                    type="checkbox"
                    checked={receive[line.id]?.selected ?? false}
                    onChange={(e) =>
                      setReceive((x) => ({
                        ...x,
                        [line.id]: {
                          ...(x[line.id] ?? { quantity: line.orderQuantity }),
                          selected: e.target.checked,
                        },
                      }))
                    }
                  />{" "}
                  Receive this line
                </label>
                <label>
                  Received quantity
                  <input
                    inputMode="decimal"
                    disabled={!receive[line.id]?.selected}
                    value={receive[line.id]?.quantity ?? line.orderQuantity}
                    onChange={(e) =>
                      setReceive((x) => ({
                        ...x,
                        [line.id]: {
                          selected: x[line.id]?.selected ?? false,
                          quantity: e.target.value,
                        },
                      }))
                    }
                  />
                </label>
              </div>
            ) : (
              <div>
                <p>
                  <strong>{line.supplierName || "Supplier not set"}</strong> · Order{" "}
                  {line.orderQuantity} {line.orderUnit}
                  {line.receivedQuantity !== null ? ` · Received ${line.receivedQuantity}` : ""}
                </p>
                {mismatch && (
                  <p className="guide-discrepancy" role="status">
                    {mismatch}
                  </p>
                )}
                {line.discrepancyKind === "none" && line.receivedQuantity !== null && (
                  <p className="guide-match" role="status">
                    All here
                  </p>
                )}
              </div>
            )}
          </article>
        );
      })}
      {ordered && (
        <div className="inventory-form-fields receive-invoice-link">
          <label>
            Link invoice <span>Optional · already uploaded</span>
            <select
              value={linkedInvoiceId}
              onChange={(e) => setLinkedInvoiceId(e.target.value)}
              disabled={Boolean(draft.linkedInvoiceId)}
            >
              <option value="">Skip for now</option>
              {invoices.map((invoice) => (
                <option key={invoice.id} value={invoice.id}>
                  {invoice.supplierName} · {invoice.invoiceDate}
                </option>
              ))}
              {draft.linkedInvoiceId &&
                !invoices.some((i) => i.id === draft.linkedInvoiceId) && (
                  <option value={draft.linkedInvoiceId}>
                    {draft.linkedInvoiceSupplierName ?? "Linked invoice"}
                    {draft.linkedInvoiceDate ? ` · ${draft.linkedInvoiceDate}` : ""}
                  </option>
                )}
            </select>
          </label>
        </div>
      )}
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
      {ordered ? (
        <button
          className="ledger-button"
          type="button"
          disabled={busy || unresolved.length === 0}
          onClick={() => void submitReceipt()}
        >
          {busy ? "Recording…" : "Record selected receipt"}
        </button>
      ) : (
        manager && (
          <div className="guide-actions">
            <button
              className="file-button"
              disabled={busy || !dirty}
              type="button"
              onClick={() => void act("save")}
            >
              {dirty ? "Save changes" : "Changes saved"}
            </button>
            <button
              className="ledger-button"
              disabled={busy}
              type="button"
              onClick={() => void act("ordered")}
            >
              {busy ? "Working…" : "Mark as ordered"}
            </button>
            <button
              className="text-button"
              disabled={busy}
              type="button"
              onClick={() => void act("cancel")}
            >
              Cancel guide
            </button>
          </div>
        )
      )}
    </section>
  );
}
