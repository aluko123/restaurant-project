import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import type { ApiRequest } from "./SalesWorkspace";
import { InventoryImportPanel } from "./InventoryImportPanel";
import { OrderGuidePanel, type OrderGuide, type SupplierOption } from "./OrderGuidePanel";

export type InventoryItem = {
  id: string;
  name: string;
  category: string | null;
  countUnit: string;
  parLevel: string | null;
  active: boolean;
  storageAreaId: string | null;
  storageAreaName: string | null;
  shelfOrder: number;
  preferredSupplierId: string | null;
  preferredSupplierName: string | null;
  latestQuantity: string | null;
  previousQuantity: string | null;
  change: string | null;
  lastCountedAt: string | null;
  lowStock: boolean;
};

type Supplier = SupplierOption & {
  archivedAt: string | null;
  createdAt: string;
  updatedAt: string;
};

type StorageArea = {
  id: string;
  name: string;
  sortOrder: number;
  active: boolean;
  itemCount: number;
};

type InventoryCountEntry = {
  id: string;
  inventoryItemId: string;
  name: string;
  category: string | null;
  countUnit: string;
  storageAreaName: string | null;
  storageAreaSort: number;
  shelfOrder: number;
  previousQuantity: string | null;
  quantity: string | null;
  skipped: boolean;
};

type InventoryCount = {
  id: string;
  status: string;
  scope: string;
  storageAreaIds: string[];
  revision: number;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  entries: InventoryCountEntry[];
};

type InventoryDraftResponse = { count: InventoryCount | null };

type CountSummary = {
  id: string;
  status: string;
  scope: string;
  revision: number;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  entryCount: number;
  countedCount: number;
  skippedCount: number;
  areaNames: string | null;
};

type ItemFields = {
  name: string;
  category: string;
  countUnit: string;
  parLevel: string;
  storageAreaId: string;
  shelfOrder: string;
  preferredSupplierId: string;
  active: boolean;
};

type Mode = "overview" | "start" | "count" | "review" | "changes" | "history" | "historyDetail" | "areas";

type EntryState = { quantity: string; skipped: boolean };

const inventoryUnits = ["each", "lb", "oz", "kg", "g", "case", "bag", "bottle", "can", "gal", "L"];
const suggestedAreas = ["Walk-in", "Dry storage", "Prep", "Bar", "Freezer"];
const countScopeLabel = (scope: string) =>
  scope === "areas" ? "Selected areas" : scope === "focused" ? "Selected items" : "Whole house";
const blankItem: ItemFields = {
  name: "",
  category: "",
  countUnit: "each",
  parLevel: "",
  storageAreaId: "",
  shelfOrder: "0",
  preferredSupplierId: "",
  active: true,
};

type Props = {
  restaurant: { role: string };
  request: ApiRequest;
};

export function InventoryWorkspace({ restaurant, request }: Props) {
  const manager = restaurant.role === "owner" || restaurant.role === "manager";
  const [items, setItems] = useState<InventoryItem[]>([]);
  const [areas, setAreas] = useState<StorageArea[]>([]);
  const [count, setCount] = useState<InventoryCount | null>(null);
  const [mode, setMode] = useState<Mode>("overview");
  const [entryState, setEntryState] = useState<Record<string, EntryState>>({});
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [fields, setFields] = useState<ItemFields>(blankItem);
  const [editing, setEditing] = useState<InventoryItem | null>(null);
  const [inventorySearch, setInventorySearch] = useState("");
  const [inventoryView, setInventoryView] = useState<"attention" | "active" | "all" | "archived">("attention");
  const [inventoryCategory, setInventoryCategory] = useState("all");
  const [inventoryArea, setInventoryArea] = useState("all");
  const [guide, setGuide] = useState<OrderGuide | null>(null);
  const [suppliers, setSuppliers] = useState<Supplier[]>([]);
  const [supplierName, setSupplierName] = useState("");
  const [editingSupplier, setEditingSupplier] = useState<Supplier | null>(null);
  const [startAreaIds, setStartAreaIds] = useState<string[]>([]);
  const [confirmSkipped, setConfirmSkipped] = useState(false);
  const [areaName, setAreaName] = useState("");
  const [editingArea, setEditingArea] = useState<StorageArea | null>(null);
  const [history, setHistory] = useState<CountSummary[]>([]);
  const [historyCursor, setHistoryCursor] = useState<{ completedAt: string; id: string } | null>(null);
  const [loadingHistoryMore, setLoadingHistoryMore] = useState(false);
  const [historyDetail, setHistoryDetail] = useState<InventoryCount | null>(null);
  const [completedForChanges, setCompletedForChanges] = useState<InventoryCount | null>(null);

  const adoptCount = (value: InventoryCount) => {
    setCount(value);
    setEntryState(
      Object.fromEntries(
        value.entries.map((entry) => [
          entry.id,
          { quantity: entry.quantity ?? "", skipped: entry.skipped },
        ]),
      ),
    );
  };

  const clearFeedback = useCallback(() => {
    setError("");
    setNotice("");
  }, []);
  const showError = useCallback((message: string) => {
    setNotice("");
    setError(message);
  }, []);
  const showNotice = useCallback((message: string) => {
    setError("");
    setNotice(message);
  }, []);

  const applyOverview = useCallback(
    (nextItems: InventoryItem[], draft: InventoryDraftResponse, openGuide: OrderGuide | null, nextAreas: StorageArea[], nextSuppliers: Supplier[]) => {
      setItems(nextItems);
      setCount(draft.count);
      setGuide(openGuide);
      setAreas(nextAreas);
      setSuppliers(nextSuppliers);
      if (draft.count) {
        setEntryState(
          Object.fromEntries(
            draft.count.entries.map((entry) => [
              entry.id,
              { quantity: entry.quantity ?? "", skipped: entry.skipped },
            ]),
          ),
        );
      }
    },
    [],
  );

  const fetchOverview = useCallback(async () => {
    const [nextItems, draft, openGuide, nextAreas, nextSuppliers] = await Promise.all([
      request<InventoryItem[]>("/v1/inventory-items"),
      request<InventoryDraftResponse>("/v1/inventory-counts/draft"),
      request<OrderGuide | null>("/v1/order-guides/open"),
      request<StorageArea[]>("/v1/storage-areas"),
      request<Supplier[]>("/v1/suppliers"),
    ]);
    return { nextItems, draft, openGuide, nextAreas, nextSuppliers };
  }, [request]);

  const softRefresh = useCallback(async () => {
    try {
      const data = await fetchOverview();
      applyOverview(data.nextItems, data.draft, data.openGuide, data.nextAreas, data.nextSuppliers);
    } catch {
      // Keep current screen state; user can hit Refresh.
    }
  }, [applyOverview, fetchOverview]);

  const loadOverview = useCallback(async () => {
    setLoading(true);
    try {
      const data = await fetchOverview();
      applyOverview(data.nextItems, data.draft, data.openGuide, data.nextAreas, data.nextSuppliers);
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "Inventory couldn't load. Try again.");
    } finally {
      setLoading(false);
    }
  }, [applyOverview, fetchOverview, showError]);

  useEffect(() => {
    void loadOverview();
  }, [loadOverview]);

  const activeAreas = useMemo(() => areas.filter((area) => area.active), [areas]);
  const active = items.filter((item) => item.active);

  function upsertSupplier(saved: Supplier) {
    setSuppliers((current) => {
      const without = current.filter((row) => row.id !== saved.id);
      return [...without, saved].sort((a, b) => a.name.localeCompare(b.name));
    });
  }

  function payload(activeValue = fields.active) {
    return {
      name: fields.name,
      category: fields.category || null,
      countUnit: fields.countUnit,
      parLevel: fields.parLevel || null,
      storageAreaId: fields.storageAreaId || null,
      shelfOrder: Number(fields.shelfOrder || "0"),
      preferredSupplierId: fields.preferredSupplierId || null,
      active: activeValue,
    };
  }

  async function saveSupplier(event: FormEvent) {
    event.preventDefault();
    if (!supplierName.trim()) {
      showError("Add a supplier name.");
      return;
    }
    setBusy(true);
    try {
      const saved = await request<Supplier>(
        editingSupplier ? `/v1/suppliers/${editingSupplier.id}` : "/v1/suppliers",
        {
          method: editingSupplier ? "PUT" : "POST",
          body: JSON.stringify({ name: supplierName.trim() }),
        },
      );
      upsertSupplier(saved);
      showNotice(
        editingSupplier
          ? `${saved.name} updated.`
          : `${saved.name} added to your suppliers.`,
      );
      setSupplierName("");
      setEditingSupplier(null);
      await softRefresh();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The supplier couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function archiveSupplier(supplier: Supplier) {
    if (!window.confirm(`Archive ${supplier.name}? Preferred settings that use it will clear.`)) {
      return;
    }
    setBusy(true);
    try {
      await request(`/v1/suppliers/${supplier.id}/archive`, { method: "POST", body: "{}" });
      setSuppliers((current) => current.filter((row) => row.id !== supplier.id));
      showNotice(`${supplier.name} archived.`);
      if (editingSupplier?.id === supplier.id) {
        setEditingSupplier(null);
        setSupplierName("");
      }
      await softRefresh();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The supplier couldn't be archived.");
    } finally {
      setBusy(false);
    }
  }

  async function saveItem(event: FormEvent) {
    event.preventDefault();
    if (!fields.name.trim() || !fields.countUnit.trim()) {
      showError("Add an item name and count unit.");
      return;
    }
    setBusy(true);
    try {
      await request(editing ? `/v1/inventory-items/${editing.id}` : "/v1/inventory-items", {
        method: editing ? "PUT" : "POST",
        body: JSON.stringify(payload()),
      });
      showNotice(editing ? `${fields.name.trim()} updated.` : `${fields.name.trim()} added.`);
      setFields(blankItem);
      setEditing(null);
      await softRefresh();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The item couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  function edit(item: InventoryItem) {
    setEditing(item);
    setFields({
      name: item.name,
      category: item.category ?? "",
      countUnit: item.countUnit,
      parLevel: item.parLevel ?? "",
      storageAreaId: item.storageAreaId ?? "",
      shelfOrder: String(item.shelfOrder ?? 0),
      preferredSupplierId: item.preferredSupplierId ?? "",
      active: item.active,
    });
    window.scrollTo({ top: 0, behavior: "smooth" });
  }

  async function toggle(item: InventoryItem) {
    setBusy(true);
    try {
      await request(`/v1/inventory-items/${item.id}`, {
        method: "PUT",
        body: JSON.stringify({
          name: item.name,
          category: item.category,
          countUnit: item.countUnit,
          parLevel: item.parLevel,
          storageAreaId: item.storageAreaId,
          shelfOrder: item.shelfOrder,
          preferredSupplierId: item.preferredSupplierId,
          active: !item.active,
        }),
      });
      showNotice(`${item.name} ${item.active ? "archived" : "reactivated"}.`);
      await softRefresh();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The item couldn't be updated.");
    } finally {
      setBusy(false);
    }
  }

  async function saveDraft(announce = true) {
    if (!count) return null;
    setBusy(true);
    if (announce) clearFeedback();
    else setError("");
    try {
      const value = await request<InventoryCount>(`/v1/inventory-counts/${count.id}`, {
        method: "PUT",
        body: JSON.stringify({
          revision: count.revision,
          entries: count.entries.map((entry) => {
            const state = entryState[entry.id] ?? { quantity: "", skipped: false };
            return {
              id: entry.id,
              quantity: state.skipped ? null : state.quantity.trim() || null,
              skipped: state.skipped,
            };
          }),
        }),
      });
      adoptCount(value);
      if (announce) showNotice("Draft saved.");
      return value;
    } catch (reason) {
      showError(
        reason instanceof Error
          ? reason.message
          : "The draft couldn't be saved. Check the quantities and try again.",
      );
      return null;
    } finally {
      setBusy(false);
    }
  }

  async function openStart() {
    clearFeedback();
    if (count) {
      setMode("count");
      return;
    }
    if (activeAreas.length === 0) {
      await startCount([]);
      return;
    }
    setStartAreaIds([]);
    setMode("start");
  }

  async function startCount(storageAreaIds: string[]) {
    setBusy(true);
    clearFeedback();
    try {
      const body =
        storageAreaIds.length > 0
          ? JSON.stringify({ storageAreaIds })
          : "{}";
      const value = await request<InventoryCount>("/v1/inventory-counts", {
        method: "POST",
        body,
      });
      adoptCount(value);
      setMode("count");
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The count couldn't start.");
    } finally {
      setBusy(false);
    }
  }

  async function reviewCount() {
    const saved = await saveDraft(false);
    if (saved) {
      setNotice("");
      setConfirmSkipped(false);
      setMode("review");
    }
  }

  async function backToOverview() {
    const saved = await saveDraft(false);
    if (saved) {
      setMode("overview");
      showNotice("Draft saved. Resume when you're ready.");
    }
  }

  async function createOrderGuide(countId?: string) {
    const next = await request<OrderGuide>("/v1/order-guides", {
      method: "POST",
      body: JSON.stringify(countId ? { countId } : {}),
    });
    if (next.status === "draft" || next.status === "ordered") {
      setGuide(next);
      return "Your order guide is ready to review.";
    }
    setGuide(null);
    return "That count already has a finished order guide. Complete another physical count to create a new one.";
  }

  async function createLatestOrderGuide() {
    setBusy(true);
    clearFeedback();
    try {
      showNotice(await createOrderGuide());
    } catch (reason) {
      showError(
        reason instanceof Error
          ? reason.message
          : "An order guide couldn't be created from the latest count.",
      );
    } finally {
      setBusy(false);
    }
  }

  async function finishAfterCount(completed: InventoryCount) {
    setCount(null);
    setEntryState({});
    const changes = completed.entries.filter((entry) =>
      isBigChange(entry.previousQuantity, entry.quantity),
    );
    if (changes.length > 0) {
      setCompletedForChanges(completed);
      setMode("changes");
      await loadOverview();
      return;
    }
    setCompletedForChanges(null);
    setMode("overview");
    let message = "Inventory count completed.";
    if (manager) {
      try {
        message = `Inventory count completed. ${await createOrderGuide(completed.id)}`;
      } catch (reason) {
        message = `Inventory count completed. ${
          reason instanceof Error ? reason.message : "No order guide was created from this count."
        }`;
      }
    }
    await loadOverview();
    showNotice(message);
  }

  async function complete() {
    if (!count) return;
    const open = count.entries.filter((entry) => {
      const state = entryState[entry.id] ?? { quantity: "", skipped: false };
      return !state.skipped && !state.quantity.trim();
    });
    if (open.length > 0) {
      showError("Finish or skip every item before completing this count.");
      return;
    }
    const skipped = count.entries.filter((entry) => entryState[entry.id]?.skipped);
    if (skipped.length > 0 && !confirmSkipped) {
      showError("Confirm the skipped items to complete this count.");
      return;
    }
    const saved = await saveDraft(false);
    if (!saved) return;
    setBusy(true);
    clearFeedback();
    try {
      const completed = await request<InventoryCount>(`/v1/inventory-counts/${saved.id}/complete`, {
        method: "POST",
        body: JSON.stringify({
          confirmSkipped: skipped.length > 0,
          revision: saved.revision,
        }),
      });
      await finishAfterCount(completed);
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The count couldn't be completed.");
    } finally {
      setBusy(false);
    }
  }

  async function discardCount() {
    if (!count) return;
    if (
      !window.confirm(
        "Discard this count? Everything entered for it is deleted, and nothing is recorded in history.",
      )
    ) {
      return;
    }
    setBusy(true);
    setError("");
    try {
      await request(`/v1/inventory-counts/${count.id}`, { method: "DELETE" });
      setCount(null);
      setEntryState({});
      setMode("overview");
      await loadOverview();
      showNotice("Count discarded.");
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The count couldn't be discarded.");
    } finally {
      setBusy(false);
    }
  }

  async function continueFromChanges() {
    const completed = completedForChanges;
    setCompletedForChanges(null);
    setMode("overview");
    let message = "Inventory count completed.";
    if (manager && completed) {
      try {
        message = `Inventory count completed. ${await createOrderGuide(completed.id)}`;
      } catch (reason) {
        message = `Inventory count completed. ${
          reason instanceof Error ? reason.message : "No order guide was created from this count."
        }`;
      }
    }
    await loadOverview();
    showNotice(message);
  }

  async function saveArea(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    clearFeedback();
    try {
      if (editingArea) {
        await request(`/v1/storage-areas/${editingArea.id}`, {
          method: "PUT",
          body: JSON.stringify({ name: areaName, active: editingArea.active }),
        });
        showNotice(`${areaName.trim()} updated.`);
      } else {
        await request("/v1/storage-areas", {
          method: "POST",
          body: JSON.stringify({ name: areaName, active: true }),
        });
        showNotice(`${areaName.trim()} added.`);
      }
      setAreaName("");
      setEditingArea(null);
      await loadOverview();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The storage area couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function addSuggestedArea(name: string) {
    if (areas.some((area) => area.name.toLocaleLowerCase() === name.toLocaleLowerCase())) {
      showNotice(`${name} is already on your list.`);
      return;
    }
    setBusy(true);
    setError("");
    try {
      await request("/v1/storage-areas", {
        method: "POST",
        body: JSON.stringify({ name, active: true }),
      });
      showNotice(`${name} added.`);
      await loadOverview();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The storage area couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function toggleArea(area: StorageArea) {
    setBusy(true);
    setError("");
    try {
      await request(`/v1/storage-areas/${area.id}`, {
        method: "PUT",
        body: JSON.stringify({ name: area.name, active: !area.active }),
      });
      showNotice(`${area.name} ${area.active ? "archived" : "reactivated"}.`);
      await loadOverview();
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The storage area couldn't be updated.");
    } finally {
      setBusy(false);
    }
  }

  async function moveArea(areaId: string, direction: -1 | 1) {
    const ordered = [...areas].sort((a, b) => a.sortOrder - b.sortOrder || a.name.localeCompare(b.name));
    const index = ordered.findIndex((area) => area.id === areaId);
    const swap = index + direction;
    if (index < 0 || swap < 0 || swap >= ordered.length) return;
    const next = [...ordered];
    [next[index], next[swap]] = [next[swap], next[index]];
    setBusy(true);
    setError("");
    try {
      const updated = await request<StorageArea[]>("/v1/storage-areas/reorder", {
        method: "PUT",
        body: JSON.stringify({ areaIds: next.map((area) => area.id) }),
      });
      setAreas(updated);
      showNotice("Walk order updated.");
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "The walk order couldn't be saved.");
    } finally {
      setBusy(false);
    }
  }

  async function openHistory() {
    setBusy(true);
    clearFeedback();
    try {
      const rows = await request<CountSummary[]>("/v1/inventory-counts");
      setHistory(rows);
      const last = rows[rows.length - 1];
      setHistoryCursor(rows.length >= 50 && last?.completedAt ? { completedAt: last.completedAt, id: last.id } : null);
      setMode("history");
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "Past counts couldn't load.");
    } finally {
      setBusy(false);
    }
  }

  async function loadMoreHistory() {
    if (!historyCursor) return;
    setLoadingHistoryMore(true);
    setError("");
    try {
      const query = `beforeCompletedAt=${encodeURIComponent(historyCursor.completedAt)}&beforeId=${historyCursor.id}`;
      const rows = await request<CountSummary[]>(`/v1/inventory-counts?${query}`);
      setHistory((current) => [...current, ...rows]);
      const last = rows[rows.length - 1];
      setHistoryCursor(rows.length >= 50 && last?.completedAt ? { completedAt: last.completedAt, id: last.id } : null);
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "Older counts couldn't load.");
    } finally {
      setLoadingHistoryMore(false);
    }
  }

  async function openHistoryDetail(id: string) {
    setBusy(true);
    setError("");
    try {
      const detail = await request<InventoryCount>(`/v1/inventory-counts/${id}`);
      setHistoryDetail(detail);
      setMode("historyDetail");
    } catch (reason) {
      showError(reason instanceof Error ? reason.message : "That count couldn't load.");
    } finally {
      setBusy(false);
    }
  }

  function setQuantity(entryId: string, quantity: string) {
    setEntryState((current) => ({
      ...current,
      [entryId]: { quantity, skipped: false },
    }));
  }

  function toggleSkip(entryId: string) {
    setEntryState((current) => {
      const prev = current[entryId] ?? { quantity: "", skipped: false };
      return {
        ...current,
        [entryId]: prev.skipped
          ? { quantity: "", skipped: false }
          : { quantity: "", skipped: true },
      };
    });
  }

  if (mode === "areas") {
    return (
      <section className="inventory-workspace">
        <button className="text-button" type="button" onClick={() => setMode("overview")}>
          ← Back to inventory
        </button>
        <header className="inventory-heading">
          <p className="section-code">Walk order</p>
          <h1>Storage areas</h1>
          <p>Count follows this walk. Put items on the shelf path they live on.</p>
        </header>
        {error && (
          <p className="form-error inventory-message" role="alert">
            {error}
          </p>
        )}
        {notice && (
          <p className="success-notice inventory-message" role="status">
            {notice}
          </p>
        )}
        {manager && (
          <form className="inventory-item-form" onSubmit={saveArea}>
            <div className="list-heading">
              <h2>{editingArea ? "Edit area" : "Add a storage area"}</h2>
              {editingArea && (
                <button
                  className="text-button"
                  type="button"
                  onClick={() => {
                    setEditingArea(null);
                    setAreaName("");
                  }}
                >
                  Cancel
                </button>
              )}
            </div>
            <div className="inventory-form-fields">
              <label>
                Name
                <input
                  required
                  maxLength={40}
                  value={areaName}
                  onChange={(event) => setAreaName(event.target.value)}
                />
              </label>
            </div>
            <button className="ledger-button" disabled={busy}>
              {busy ? "Saving…" : editingArea ? "Save area" : "Add area"}
            </button>
            {!editingArea && (
              <div className="suggested-chips" aria-label="Suggested storage areas">
                {suggestedAreas.map((name) => (
                  <button
                    key={name}
                    className="file-button"
                    type="button"
                    disabled={busy}
                    onClick={() => void addSuggestedArea(name)}
                  >
                    {name}
                  </button>
                ))}
              </div>
            )}
          </form>
        )}
        {areas.length === 0 ? (
          <p className="empty-state">
            {manager
              ? "No storage areas yet. Add where you store food so counts follow the walk."
              : "No storage areas yet. Ask a manager to set them up."}
          </p>
        ) : (
          <div className="storage-area-list">
            {areas.map((area, index) => (
              <article className={`storage-area-card${!area.active ? " inactive" : ""}`} key={area.id}>
                <div>
                  <h3>{area.name}</h3>
                  <p>
                    {area.itemCount} active {area.itemCount === 1 ? "item" : "items"}
                    {!area.active ? " · Archived" : ""}
                  </p>
                </div>
                {manager && (
                  <div className="card-actions">
                    <button
                      className="text-button"
                      type="button"
                      disabled={busy || index === 0}
                      onClick={() => void moveArea(area.id, -1)}
                    >
                      Up
                    </button>
                    <button
                      className="text-button"
                      type="button"
                      disabled={busy || index === areas.length - 1}
                      onClick={() => void moveArea(area.id, 1)}
                    >
                      Down
                    </button>
                    <button
                      className="file-button"
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        setEditingArea(area);
                        setAreaName(area.name);
                      }}
                    >
                      Edit
                    </button>
                    <button
                      className="text-button"
                      type="button"
                      disabled={busy}
                      onClick={() => void toggleArea(area)}
                    >
                      {area.active ? "Archive" : "Reactivate"}
                    </button>
                  </div>
                )}
              </article>
            ))}
          </div>
        )}
      </section>
    );
  }

  if (mode === "history") {
    return (
      <section className="inventory-workspace">
        <button className="text-button" type="button" onClick={() => setMode("overview")}>
          ← Back to inventory
        </button>
        <header className="inventory-heading">
          <p className="section-code">Count history</p>
          <h1>Past counts</h1>
          <p>Completed physical counts stay here for reference.</p>
        </header>
        {error && (
          <p className="form-error inventory-message" role="alert">
            {error}
          </p>
        )}
        {history.length === 0 ? (
          <p className="empty-state">No completed counts yet. Finish a count to build history.</p>
        ) : (
          <>
            {historyCursor && (
              <button
                className="file-button"
                type="button"
                disabled={loadingHistoryMore || busy}
                onClick={() => void loadMoreHistory()}
              >
                {loadingHistoryMore ? "Loading older counts…" : "Load older counts"}
              </button>
            )}
            <div className="count-history-list">
            {history.map((row) => (
              <article className="count-history-card" key={row.id}>
                <div>
                  <h3>{row.completedAt ? formatInventoryDate(row.completedAt) : "Completed count"}</h3>
                  <p>
                    {row.scope === "areas" && row.areaNames
                      ? row.areaNames
                      : countScopeLabel(row.scope)}{" "}
                    · {row.countedCount} counted
                    {row.skippedCount > 0 ? ` · ${row.skippedCount} skipped` : ""} · {row.entryCount}{" "}
                    items
                  </p>
                </div>
                <button
                  className="file-button"
                  type="button"
                  disabled={busy}
                  onClick={() => void openHistoryDetail(row.id)}
                >
                  View
                </button>
              </article>
            ))}
            </div>
          </>
        )}
      </section>
    );
  }

  if (mode === "historyDetail" && historyDetail) {
    const groups = groupByArea(historyDetail.entries);
    return (
      <section className="inventory-workspace count-workspace">
        <button className="text-button" type="button" onClick={() => setMode("history")}>
          ← Back to past counts
        </button>
        <header className="inventory-heading">
          <p className="section-code">Count detail</p>
          <h1>
            {historyDetail.completedAt
              ? formatInventoryDate(historyDetail.completedAt)
              : "Completed count"}
          </h1>
          <p>
            {countScopeLabel(historyDetail.scope)} ·{" "}
            {historyDetail.entries.filter((e) => e.quantity !== null).length} counted ·{" "}
            {historyDetail.entries.filter((e) => e.skipped).length} skipped
          </p>
        </header>
        {groups.map(([area, entries]) => (
          <section className="count-category" key={area}>
            <h2>{area}</h2>
            {entries.map((entry) => (
              <div className="count-row history-row" key={entry.id}>
                <span>
                  <strong>{entry.name}</strong>
                  <small>
                    {entry.skipped
                      ? "Skipped"
                      : entry.quantity === null
                        ? "No quantity"
                        : `${formatInventoryNumber(entry.quantity)} ${entry.countUnit}`}
                    {entry.previousQuantity !== null
                      ? ` · was ${formatInventoryNumber(entry.previousQuantity)}`
                      : ""}
                  </small>
                </span>
              </div>
            ))}
          </section>
        ))}
      </section>
    );
  }

  if (mode === "start") {
    return (
      <section className="inventory-workspace count-workspace">
        <button className="text-button" type="button" onClick={() => setMode("overview")}>
          ← Back to inventory
        </button>
        <header className="inventory-heading">
          <p className="section-code">Start count</p>
          <h1>What are you counting?</h1>
          <p>Count the whole house, or walk selected storage areas only.</p>
        </header>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <div className="start-count-options">
          <button
            className="ledger-button"
            type="button"
            disabled={busy}
            onClick={() => void startCount([])}
          >
            {busy ? "Starting…" : "Whole house"}
          </button>
          {activeAreas.length > 0 && (
            <div className="area-picker">
              <h2>Or pick areas</h2>
              {activeAreas.map((area) => {
                const checked = startAreaIds.includes(area.id);
                return (
                  <label className="active-toggle" key={area.id}>
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() =>
                        setStartAreaIds((current) =>
                          checked
                            ? current.filter((id) => id !== area.id)
                            : [...current, area.id],
                        )
                      }
                    />
                    {area.name}
                    <span>
                      {area.itemCount} {area.itemCount === 1 ? "item" : "items"}
                    </span>
                  </label>
                );
              })}
              <button
                className="file-button"
                type="button"
                disabled={busy || startAreaIds.length === 0}
                onClick={() => void startCount(startAreaIds)}
              >
                {busy ? "Starting…" : `Count ${startAreaIds.length || ""} selected`}
              </button>
            </div>
          )}
        </div>
      </section>
    );
  }

  if (mode === "changes" && completedForChanges) {
    const changes = completedForChanges.entries.filter((entry) =>
      isBigChange(entry.previousQuantity, entry.quantity),
    );
    return (
      <section className="inventory-workspace count-workspace">
        <header className="inventory-heading">
          <p className="section-code">After count</p>
          <h1>Big changes</h1>
          <p>These moved a lot since the last count. Review before ordering.</p>
        </header>
        <div className="big-change-list">
          {changes.map((entry) => {
            const prev = Number(entry.previousQuantity);
            const qty = Number(entry.quantity);
            const delta = qty - prev;
            return (
              <article className="big-change-card" key={entry.id}>
                <h3>{entry.name}</h3>
                <p>
                  {formatInventoryNumber(entry.previousQuantity!)} →{" "}
                  {formatInventoryNumber(entry.quantity!)} {entry.countUnit}
                </p>
                <strong>
                  {delta > 0 ? "+" : ""}
                  {formatInventoryNumber(String(delta))} {entry.countUnit}
                </strong>
              </article>
            );
          })}
        </div>
        <div className="count-actions">
          <button
            className="ledger-button"
            type="button"
            disabled={busy}
            onClick={() => void continueFromChanges()}
          >
            {busy ? "Continuing…" : manager ? "Continue to order guide" : "Done"}
          </button>
        </div>
      </section>
    );
  }

  if ((mode === "count" || mode === "review") && count) {
    const groups = groupByArea(count.entries);
    const open = count.entries.filter((entry) => {
      const state = entryState[entry.id] ?? { quantity: "", skipped: false };
      return !state.skipped && !state.quantity.trim();
    });
    const skipped = count.entries.filter((entry) => entryState[entry.id]?.skipped);
    const counted = count.entries.length - open.length - skipped.length;
    const scopeLabel =
      count.scope === "areas"
        ? count.entries
            .map((e) => e.storageAreaName)
            .filter((name, index, all): name is string => Boolean(name) && all.indexOf(name) === index)
            .join(" · ") || "Selected areas"
        : countScopeLabel(count.scope);

    if (mode === "review") {
      return (
        <section className="inventory-workspace count-workspace">
          <button className="text-button" type="button" onClick={() => setMode("count")}>
            ← Back to count
          </button>
          <header className="inventory-heading">
            <p className="section-code">Inventory review</p>
            <h1>Review count</h1>
            <p>
              {counted} counted · {skipped.length} skipped · {open.length} still open · {scopeLabel}
            </p>
          </header>
          {open.length > 0 && (
            <div className="missing-list">
              <h2>Still open</h2>
              <p>Finish or skip these before completing.</p>
              <ul>
                {open.map((entry) => (
                  <li key={entry.id}>
                    <strong>{entry.name}</strong> · {entry.countUnit}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {skipped.length > 0 && (
            <div className="missing-list">
              <h2>Skipped items</h2>
              <p>These stay blank for this count.</p>
              <ul>
                {skipped.map((entry) => (
                  <li key={entry.id}>
                    <strong>{entry.name}</strong> · {entry.countUnit}
                  </li>
                ))}
              </ul>
              <label className="active-toggle confirm-skipped">
                <input
                  type="checkbox"
                  checked={confirmSkipped}
                  onChange={(event) => setConfirmSkipped(event.target.checked)}
                />
                I skipped these on purpose
              </label>
            </div>
          )}
          {error && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
          <div className="count-actions">
            <button className="file-button" type="button" onClick={() => setMode("count")}>
              Back to count
            </button>
            <button
              className="ledger-button"
              type="button"
              disabled={busy || open.length > 0 || (skipped.length > 0 && !confirmSkipped)}
              onClick={() => void complete()}
            >
              {busy
                ? "Completing…"
                : skipped.length
                  ? "Complete with skipped items"
                  : "Complete count"}
            </button>
          </div>
        </section>
      );
    }

    return (
      <section className="inventory-workspace count-workspace">
        <button className="text-button" type="button" disabled={busy} onClick={() => void backToOverview()}>
          ← Save and return to overview
        </button>
        <header className="inventory-heading">
          <p className="section-code">Inventory count</p>
          <h1>Record a physical count</h1>
          <p>
            {scopeLabel}. {counted + skipped.length} of {count.entries.length} done
            {skipped.length > 0 ? ` · ${skipped.length} skipped` : ""}. Draft stays when you leave.
          </p>
        </header>
        {groups.map(([area, entries]) => (
          <section className="count-category" key={area}>
            <h2>{area}</h2>
            {entries.map((entry) => {
              const state = entryState[entry.id] ?? { quantity: "", skipped: false };
              return (
                <div className={`count-row${state.skipped ? " skipped" : ""}`} key={entry.id}>
                  <span>
                    <strong>{entry.name}</strong>
                    <small>
                      Count in {entry.countUnit}
                      {entry.previousQuantity !== null
                        ? ` · Last: ${formatInventoryNumber(entry.previousQuantity)} ${entry.countUnit}`
                        : " · Not counted before"}
                    </small>
                  </span>
                  <span className="quantity-field-wrap">
                    <span className="quantity-field">
                      <input
                        aria-label={`${entry.name}, quantity in ${entry.countUnit}`}
                        inputMode="decimal"
                        disabled={state.skipped}
                        value={state.skipped ? "" : state.quantity}
                        onChange={(event) => setQuantity(entry.id, event.target.value)}
                      />
                      <b>{entry.countUnit}</b>
                    </span>
                    <button
                      className={`text-button skip-button${state.skipped ? " active" : ""}`}
                      type="button"
                      onClick={() => toggleSkip(entry.id)}
                    >
                      {state.skipped ? "Undo skip" : "Skip"}
                    </button>
                  </span>
                </div>
              );
            })}
          </section>
        ))}
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
        <div className="count-actions">
          <button className="file-button" type="button" disabled={busy} onClick={() => void saveDraft()}>
            {busy ? "Saving…" : "Save draft"}
          </button>
          <button className="ledger-button" type="button" disabled={busy} onClick={() => void reviewCount()}>
            Review count
          </button>
          <button
            className="text-button"
            type="button"
            disabled={busy}
            onClick={() => void discardCount()}
          >
            Discard count
          </button>
        </div>
      </section>
    );
  }

  const normalizedInventorySearch = inventorySearch.trim().toLocaleLowerCase();
  const inventoryCategories = categoryOptions(items);
  const filteredItems = items.filter((item) => {
    const matchesSearch =
      !normalizedInventorySearch || item.name.toLocaleLowerCase().includes(normalizedInventorySearch);
    const matchesCategory =
      inventoryCategory === "all" || categoryName(item.category) === inventoryCategory;
    const matchesArea =
      inventoryArea === "all"
        ? true
        : inventoryArea === "unassigned"
          ? !item.storageAreaId
          : item.storageAreaName === inventoryArea;
    const matchesView =
      inventoryView === "attention"
        ? item.active && (item.lowStock || item.lastCountedAt === null)
        : inventoryView === "active"
          ? item.active
          : inventoryView === "archived"
            ? !item.active
            : true;
    return matchesSearch && matchesCategory && matchesArea && matchesView;
  });
  const inventoryFiltersActive =
    inventorySearch.trim() !== "" ||
    inventoryCategory !== "all" ||
    inventoryArea !== "all" ||
    inventoryView !== "attention";
  const areaFilterOptions = [
    ...activeAreas.map((area) => area.name),
    ...(items.some((item) => !item.storageAreaId) ? ["Unassigned"] : []),
  ];

  return (
    <section className="inventory-workspace">
      <header className="inventory-heading">
        <h1>Inventory</h1>
        <p>Review the last counted quantities and keep the next count moving.</p>
        <div className="inventory-header-actions">
          <button
            className="ledger-button"
            type="button"
            disabled={busy || (!count && active.length === 0)}
            onClick={() => void openStart()}
          >
            {busy ? "Opening…" : count ? "Resume count" : "Start count"}
          </button>
          <button className="file-button" type="button" disabled={busy} onClick={() => setMode("areas")}>
            Storage areas
          </button>
          <button className="text-button" type="button" disabled={busy} onClick={() => void openHistory()}>
            Past counts
          </button>
        </div>
        {count && (
          <p className="draft-badge" role="status">
            Draft in progress · {countScopeLabel(count.scope)} ·{" "}
            {count.entries.filter((e) => e.quantity !== null || e.skipped).length} of{" "}
            {count.entries.length} done
            {manager && (
              <>
                {" · "}
                <button
                  className="text-button"
                  type="button"
                  disabled={busy}
                  onClick={() => void discardCount()}
                >
                  Discard draft
                </button>
              </>
            )}
          </p>
        )}
      </header>
      {error && (
        <p className="form-error inventory-message" role="alert">
          {error}
        </p>
      )}
      {notice && (
        <p className="success-notice inventory-message" role="status">
          {notice}
        </p>
      )}
      {guide && (
        <OrderGuidePanel
          guide={guide}
          manager={manager}
          request={request}
          suppliers={suppliers}
          onChange={(next, message) => {
            setGuide(next);
            if (message) showNotice(message);
          }}
        />
      )}
      {manager && (
        <section className="suppliers-panel" aria-labelledby="suppliers-heading">
          <div className="list-heading">
            <div>
              <p className="section-code">Who you order from</p>
              <h2 id="suppliers-heading">Suppliers</h2>
            </div>
          </div>
          <p>
            Names come from invoices you upload and approve. Use them on order guides and as preferred
            suppliers on items.
          </p>
          {suppliers.length === 0 ? (
            <p className="empty-state">
              No suppliers yet. Upload an invoice to add who you order from, or add one manually below.
            </p>
          ) : (
            <div className="supplier-cards">
              {suppliers.map((supplier) => (
                <article className="supplier-card" key={supplier.id}>
                  <h3>{supplier.name}</h3>
                  <div className="card-actions">
                    <button
                      className="file-button"
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        setEditingSupplier(supplier);
                        setSupplierName(supplier.name);
                      }}
                    >
                      Rename
                    </button>
                    <button
                      className="text-button"
                      type="button"
                      disabled={busy}
                      onClick={() => void archiveSupplier(supplier)}
                    >
                      Archive
                    </button>
                  </div>
                </article>
              ))}
            </div>
          )}
          <form className="inventory-item-form suppliers-form" onSubmit={saveSupplier}>
            <div className="list-heading">
              <h3>{editingSupplier ? "Rename supplier" : "Add one not on an invoice yet"}</h3>
              {editingSupplier && (
                <button
                  className="text-button"
                  type="button"
                  onClick={() => {
                    setEditingSupplier(null);
                    setSupplierName("");
                  }}
                >
                  Cancel
                </button>
              )}
            </div>
            <div className="inventory-form-fields">
              <label>
                Name
                <input
                  required
                  maxLength={120}
                  value={supplierName}
                  onChange={(e) => setSupplierName(e.target.value)}
                  placeholder="Local farm, cash-and-carry, …"
                />
              </label>
            </div>
            <button className="file-button" disabled={busy}>
              {busy ? "Saving…" : editingSupplier ? "Save supplier" : "Add supplier"}
            </button>
          </form>
        </section>
      )}
      {manager && !guide && !count && items.some((item) => item.lastCountedAt) && (
        <section className="order-guide-prompt">
          <div>
            <p className="section-code">Purchasing</p>
            <h2>Create an order guide</h2>
            <p>Use the latest completed count and current par levels to see what may need ordering.</p>
          </div>
          <button
            className="file-button"
            type="button"
            disabled={busy}
            onClick={() => void createLatestOrderGuide()}
          >
            {busy ? "Checking…" : "Use latest count"}
          </button>
        </section>
      )}
      {manager && (
        <InventoryImportPanel
          request={request}
          onApplied={async (appliedImport) => {
            if (count) {
              await loadOverview();
              showNotice(
                "Inventory items imported. They will enter the next count because a draft is already in progress.",
              );
              return;
            }
            try {
              const itemIds = appliedImport.rows.flatMap((row) =>
                row.createdInventoryItemId ? [row.createdInventoryItemId] : [],
              );
              const next = await request<InventoryCount>("/v1/inventory-counts", {
                method: "POST",
                body: JSON.stringify({ itemIds }),
              });
              adoptCount(next);
              showNotice("Inventory items imported. Your first count is ready to record.");
              setMode("count");
            } catch (reason) {
              showError(
                `Inventory items were imported, but the first count could not start. ${
                  reason instanceof Error
                    ? reason.message
                    : "Start it from Inventory when you're ready."
                }`,
              );
            }
          }}
        />
      )}
      {manager && (
        <form className="inventory-item-form" onSubmit={saveItem}>
          <div className="list-heading">
            <h2>{editing ? "Edit item" : "Add one item"}</h2>
            {editing && (
              <button
                className="text-button"
                type="button"
                onClick={() => {
                  setEditing(null);
                  setFields(blankItem);
                }}
              >
                Cancel
              </button>
            )}
          </div>
          <div className="inventory-form-fields">
            <label>
              Name
              <input
                required
                maxLength={50}
                value={fields.name}
                onChange={(e) => setFields({ ...fields, name: e.target.value })}
              />
            </label>
            <label>
              Category <span>Optional</span>
              <input
                maxLength={20}
                value={fields.category}
                onChange={(e) => setFields({ ...fields, category: e.target.value })}
              />
            </label>
            <label>
              Count unit
              <select
                required
                value={fields.countUnit}
                onChange={(e) => setFields({ ...fields, countUnit: e.target.value })}
              >
                {inventoryUnits.map((unit) => (
                  <option key={unit} value={unit}>
                    {unit}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Par level <span>Optional</span>
              <input
                inputMode="decimal"
                value={fields.parLevel}
                onChange={(e) => setFields({ ...fields, parLevel: e.target.value })}
              />
            </label>
            <label>
              Preferred supplier <span>Optional</span>
              <select
                value={fields.preferredSupplierId}
                onChange={(e) => setFields({ ...fields, preferredSupplierId: e.target.value })}
              >
                <option value="">None</option>
                {suppliers.map((supplier) => (
                  <option key={supplier.id} value={supplier.id}>
                    {supplier.name}
                  </option>
                ))}
                {fields.preferredSupplierId &&
                  !suppliers.some((s) => s.id === fields.preferredSupplierId) &&
                  editing?.preferredSupplierName && (
                    <option value={fields.preferredSupplierId}>
                      {editing.preferredSupplierName}
                    </option>
                  )}
              </select>
            </label>
            <label>
              Storage area <span>Optional</span>
              <select
                value={fields.storageAreaId}
                onChange={(e) => setFields({ ...fields, storageAreaId: e.target.value })}
              >
                <option value="">Unassigned</option>
                {activeAreas.map((area) => (
                  <option key={area.id} value={area.id}>
                    {area.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Shelf order <span>Lower is earlier</span>
              <input
                inputMode="numeric"
                value={fields.shelfOrder}
                onChange={(e) => setFields({ ...fields, shelfOrder: e.target.value })}
              />
            </label>
          </div>
          {editing && (
            <label className="active-toggle">
              <input
                type="checkbox"
                checked={fields.active}
                onChange={(e) => setFields({ ...fields, active: e.target.checked })}
              />{" "}
              Active item
            </label>
          )}
          <button className="ledger-button" disabled={busy}>
            {busy ? "Saving…" : editing ? "Save item" : "Add item"}
          </button>
        </form>
      )}
      <div className="inventory-list">
        <div className="list-heading">
          <h2>Inventory items</h2>
          <button className="text-button" type="button" onClick={() => void loadOverview()}>
            Refresh
          </button>
        </div>
        {!loading && items.length > 0 && (
          <div className="collection-toolbar" aria-label="Filter inventory items">
            <label className="collection-search">
              Search all inventory
              <input
                type="search"
                placeholder="Item name"
                value={inventorySearch}
                onChange={(event) => {
                  const value = event.target.value;
                  if (!inventorySearch.trim() && value.trim()) {
                    setInventoryView("all");
                    setInventoryCategory("all");
                    setInventoryArea("all");
                  }
                  setInventorySearch(value);
                }}
              />
            </label>
            <label>
              View
              <select
                value={inventoryView}
                onChange={(event) => setInventoryView(event.target.value as typeof inventoryView)}
              >
                <option value="attention">Needs attention</option>
                <option value="active">Active items</option>
                <option value="all">All items</option>
                <option value="archived">Archived</option>
              </select>
            </label>
            <label>
              Storage area
              <select value={inventoryArea} onChange={(event) => setInventoryArea(event.target.value)}>
                <option value="all">All areas</option>
                {areaFilterOptions.map((name) => (
                  <option key={name} value={name === "Unassigned" ? "unassigned" : name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Category
              <select
                value={inventoryCategory}
                onChange={(event) => setInventoryCategory(event.target.value)}
              >
                <option value="all">All categories ({items.length})</option>
                {inventoryCategories.map((option) => (
                  <option key={option.name} value={option.name}>
                    {option.name} ({option.count})
                  </option>
                ))}
              </select>
            </label>
            <div className="collection-toolbar-summary">
              <strong>
                {filteredItems.length} {filteredItems.length === 1 ? "item" : "items"}
              </strong>
              {inventoryFiltersActive && (
                <button
                  className="text-button"
                  type="button"
                  onClick={() => {
                    setInventorySearch("");
                    setInventoryView("attention");
                    setInventoryCategory("all");
                    setInventoryArea("all");
                  }}
                >
                  Clear filters
                </button>
              )}
            </div>
          </div>
        )}
        {loading ? (
          <p role="status">Loading inventory…</p>
        ) : items.length === 0 ? (
          <p className="empty-state">
            {manager
              ? "No inventory items yet. Add your first item above, including how the crew counts it."
              : "No inventory items are ready to count. Ask an owner or manager to add them."}
          </p>
        ) : filteredItems.length === 0 ? (
          <div className="filtered-empty">
            <h3>
              {inventoryView === "attention" &&
              !inventorySearch.trim() &&
              inventoryCategory === "all" &&
              inventoryArea === "all"
                ? "Nothing needs attention"
                : "No items match these filters"}
            </h3>
            <p>
              {inventoryView === "attention" &&
              !inventorySearch.trim() &&
              inventoryCategory === "all" &&
              inventoryArea === "all"
                ? "No active items are below par or waiting for their first count."
                : "Try another area, category, or view, or clear the current filters."}
            </p>
            <button
              className="file-button"
              type="button"
              onClick={() => {
                setInventorySearch("");
                setInventoryView("all");
                setInventoryCategory("all");
                setInventoryArea("all");
              }}
            >
              Show all inventory
            </button>
          </div>
        ) : (
          groupByAreaItems(filteredItems).map(([area, group]) => (
            <InventoryAreaGroup
              key={area}
              area={area}
              items={group}
              manager={manager}
              busy={busy}
              onEdit={edit}
              onToggle={toggle}
            />
          ))
        )}
      </div>
    </section>
  );
}

function categoryName(value: string | null) {
  return value?.trim() || "Uncategorized";
}

function areaName(value: string | null | undefined) {
  return value?.trim() || "Unassigned";
}

function categoryOptions<T extends { category: string | null }>(values: T[]) {
  const counts = new Map<string, number>();
  values.forEach((value) => {
    const name = categoryName(value.category);
    counts.set(name, (counts.get(name) ?? 0) + 1);
  });
  return [...counts]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function groupByArea(values: InventoryCountEntry[]): [string, InventoryCountEntry[]][] {
  const groups = new Map<string, InventoryCountEntry[]>();
  values.forEach((value) => {
    const key = areaName(value.storageAreaName);
    groups.set(key, [...(groups.get(key) ?? []), value]);
  });
  return [...groups.entries()];
}

function groupByAreaItems(values: InventoryItem[]): [string, InventoryItem[]][] {
  const groups = new Map<string, InventoryItem[]>();
  values.forEach((value) => {
    const key = areaName(value.storageAreaName);
    groups.set(key, [...(groups.get(key) ?? []), value]);
  });
  return [...groups.entries()];
}

function InventoryAreaGroup({
  area,
  items,
  manager,
  busy,
  onEdit,
  onToggle,
}: {
  area: string;
  items: InventoryItem[];
  manager: boolean;
  busy: boolean;
  onEdit: (item: InventoryItem) => void;
  onToggle: (item: InventoryItem) => void;
}) {
  return (
    <section className="inventory-category">
      <h3>{area}</h3>
      <div className="inventory-cards">
        {items.map((item) => (
          <article className={`inventory-card${item.lowStock ? " low-stock" : ""}`} key={item.id}>
            <div className="inventory-card-head">
              <div>
                <h4>{item.name}</h4>
                {!item.active ? (
                  <strong className="archived-label">Archived</strong>
                ) : item.lowStock ? (
                  <strong className="low-stock-label">Below par at last count</strong>
                ) : null}
              </div>
              <p className="current-quantity">
                {item.latestQuantity === null
                  ? "Not counted"
                  : `${formatInventoryNumber(item.latestQuantity)} ${item.countUnit}`}
              </p>
            </div>
            <div className="inventory-metrics">
              <p>
                <span>Previous</span>
                {item.previousQuantity === null
                  ? "—"
                  : `${formatInventoryNumber(item.previousQuantity)} ${item.countUnit}`}
              </p>
              <p>
                <span>Change</span>
                {item.change === null
                  ? "—"
                  : `${formatSigned(item.change)} ${item.countUnit}`}
              </p>
              <p>
                <span>Last counted</span>
                {item.lastCountedAt ? formatInventoryDate(item.lastCountedAt) : "Not yet"}
              </p>
              <p>
                <span>Preferred</span>
                {item.preferredSupplierName ?? "—"}
              </p>
            </div>
            {manager && (
              <div className="card-actions">
                <button className="file-button" type="button" disabled={busy} onClick={() => onEdit(item)}>
                  Edit
                </button>
                <button
                  className="text-button"
                  type="button"
                  disabled={busy}
                  onClick={() => void onToggle(item)}
                >
                  {item.active ? "Archive" : "Reactivate"}
                </button>
              </div>
            )}
          </article>
        ))}
      </div>
    </section>
  );
}

function isBigChange(previous: string | null, quantity: string | null) {
  if (previous === null || quantity === null) return false;
  const prev = Number(previous);
  const qty = Number(quantity);
  if (!Number.isFinite(prev) || !Number.isFinite(qty)) return false;
  const delta = Math.abs(qty - prev);
  return delta >= 1 && delta >= Math.abs(prev) * 0.25;
}

function formatInventoryNumber(value: string) {
  const number = Number(value);
  return Number.isFinite(number)
    ? new Intl.NumberFormat(undefined, { maximumFractionDigits: 6 }).format(number)
    : value;
}

function formatSigned(value: string) {
  const number = Number(value);
  if (!Number.isFinite(number)) return value;
  return `${number > 0 ? "+" : ""}${formatInventoryNumber(value)}`;
}

function formatInventoryDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
}
