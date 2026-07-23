import { FormEvent, useCallback, useEffect, useState } from "react";
import { useAuth } from "@workos-inc/authkit-react";
import { SalesWorkspace, type ApiRequest } from "./SalesWorkspace";
import { TodayWorkspace } from "./TodayWorkspace";
import { WeeklyBriefWorkspace } from "./WeeklyBriefWorkspace";

type AppProps = {
  authConfigured: boolean;
};

type Restaurant = { id: string; name: string; city: string; serviceStyle: ServiceStyle; timezone: string; role: string };
type Invoice = { id: string; supplierName: string; invoiceDate: string; originalFilename: string; contentType: string; sizeBytes: number; status: string; delayed: boolean; priceChangeCount: number; createdAt: string };
type ReviewLine = { id?: string; sku: string | null; description: string; quantity: string | null; unit: string | null; unitPrice: string | null; lineTotal: string | null; hasWarnings: boolean };
type Review = { invoiceId: string; supplierName: string; invoiceNumber: string | null; invoiceDate: string | null; currency: string; subtotal: string | null; tax: string | null; fees: string | null; discount: string | null; total: string | null; hasWarnings: boolean; lineItems: ReviewLine[] };
type PriceChange = { id: string; description: string; unit: string | null; currency: string; previousUnitPrice: string; currentUnitPrice: string; percentageChange: string; previousInvoiceDate: string };
type PurchaseInvoice = { invoiceId: string; supplierName: string; invoiceNumber: string | null; invoiceDate: string; currency: string };
type PurchaseInventoryItem = { id: string; name: string; category: string | null; countUnit: string };
type PurchaseLine = { id: string; position: number; sku: string | null; description: string; quantity: string | null; unit: string | null; unitPrice: string | null; lineTotal: string | null; canTrack: boolean; suggestedInventoryItemId: string | null; suggestedConversion: string | null };
type ReceiptLine = { id: string; position: number; resolution: "matched" | "created" | "ignored"; sku: string | null; description: string; quantity: string | null; unit: string | null; unitPrice: string | null; lineTotal: string | null; inventoryItemId: string | null; inventoryItemName: string | null; countUnit: string | null; conversion: string | null };
type PurchaseReceipt = { invoice: PurchaseInvoice; recordedAt: string; lines: ReceiptLine[] };
type PurchaseResponse = { status: "pending"; invoice: PurchaseInvoice; inventoryItems: PurchaseInventoryItem[]; lines: PurchaseLine[] } | { status: "recorded"; receipt: PurchaseReceipt; alreadyRecorded: boolean };
type PurchaseDecision = { action: "" } | { action: "ignore" } | { action: "match"; inventoryItemId: string; expectedCountUnit: string; conversion: string; suggested: boolean } | { action: "create"; name: string; category: string; countUnit: string; conversion: string };
type MenuItem = { id: string; name: string; category: string | null; sellingPrice: string; currency: string; active: boolean; ingredientCount: number };
type MenuImport = { id: string; originalFilename: string; status: string; delayed: boolean; createdAt: string };
type MenuImportItem = { id?: string; name: string; category: string | null; sellingPrice: string | null; currency: string | null; hasWarnings: boolean; selected?: boolean };
type CostingInventoryItem = { id: string; name: string; category: string | null; countUnit: string };
type CostSource = { invoiceId: string; sourceLineId: string; supplierName: string; invoiceDate: string; recordedAt: string; currency: string; description: string; purchaseQuantity: string | null; purchaseUnit: string | null; lineTotal: string | null; unitPrice: string | null; countUnit: string | null; countUnitsPerPurchaseUnit: string | null };
type CostArithmetic = { priceBasis: "lineTotalDividedByPurchaseQuantity" | "unitPrice"; purchaseUnitCost: string; ingredientQuantityInCountUnit: string; formula: string };
type IngredientCalculation = { status: "available"; costPerServing: string; currency: string; source: CostSource; arithmetic: CostArithmetic } | { status: "unavailable"; reason: "noRecordedReceipt" | "currencyMismatch" | "unsupportedReceiptUnit" | "incompatibleUnits" | "invalidConversion" | "unusablePrice"; recovery: string; source: CostSource | null };
type CostingIngredient = { id: string; inventoryItemId: string; inventoryItemName: string; inventoryItemCategory: string | null; inventoryItemActive: boolean; quantity: string; unit: ServingUnit; calculation: IngredientCalculation };
type CostingSummary = { status: "complete"; knownSubtotal: string; currency: string; approximateIngredientCostPercentage: string; configuredIngredientCount: number; knownIngredientCount: number } | { status: "partial"; knownSubtotal: string; currency: string; configuredIngredientCount: number; knownIngredientCount: number } | { status: "unavailable"; currency: string; configuredIngredientCount: number; knownIngredientCount: number };
type CostingResponse = { menuItem: { id: string; name: string; sellingPrice: string; currency: string }; inventoryItems: CostingInventoryItem[]; ingredients: CostingIngredient[]; summary: CostingSummary };
type ServingUnit = "g" | "kg" | "oz" | "lb" | "mL" | "L" | "fl_oz_us" | "gal_us" | "each";
type IngredientDraft = { inventoryItemId: string; quantity: string; unit: ServingUnit; archived: boolean; inventoryItemName: string };
type InventoryItem = { id: string; name: string; category: string | null; countUnit: string; parLevel: string | null; active: boolean; latestQuantity: string | null; previousQuantity: string | null; change: string | null; lastCountedAt: string | null; lowStock: boolean };
type InventoryCountEntry = { id: string; inventoryItemId: string; name: string; category: string | null; countUnit: string; quantity: string | null };
type InventoryCount = { id: string; status: string; revision: number; createdAt: string; updatedAt: string; completedAt: string | null; entries: InventoryCountEntry[] };
type InventoryDraftResponse = { count: InventoryCount | null };
type LossEventType = "waste" | "stockout";
type LossEvent = { id: string; inventoryItemId: string; eventType: LossEventType; inventoryItemName: string; countUnit: string; quantity: string | null; severity: string | null; reason: string; note: string | null; createdAt: string };
type ServiceStyle = "counter_service" | "full_service" | "fast_casual" | "cafe_bakery" | "bar";
type AppState = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; restaurant: Restaurant | null };

const serviceStyles: { value: ServiceStyle; label: string }[] = [
  { value: "counter_service", label: "Counter service" },
  { value: "full_service", label: "Full service" },
  { value: "fast_casual", label: "Fast casual" },
  { value: "cafe_bakery", label: "Cafe/Bakery" },
  { value: "bar", label: "Bar" },
];

const servingUnits: { value: ServingUnit; label: string }[] = [
  { value: "g", label: "g" },
  { value: "kg", label: "kg" },
  { value: "oz", label: "oz" },
  { value: "lb", label: "lb" },
  { value: "mL", label: "mL" },
  { value: "L", label: "L" },
  { value: "fl_oz_us", label: "US fl oz" },
  { value: "gal_us", label: "US gal" },
  { value: "each", label: "each" },
];

export function App({ authConfigured }: AppProps) {
  return authConfigured ? <AuthenticatedApp /> : <Welcome authConfigured={false} />;
}

function AuthenticatedApp() {
  const { isLoading, user, signIn, signUp, signOut, getAccessToken } = useAuth();
  const [appState, setAppState] = useState<AppState>({ status: "loading" });
  type Workspace = "today" | "brief" | "invoices" | "sales" | "menu" | "inventory" | "losses";
  const workspaceForPath = (): Workspace => window.location.pathname === "/brief" ? "brief" : window.location.pathname === "/invoices" ? "invoices" : window.location.pathname === "/sales" ? "sales" : window.location.pathname === "/menu" ? "menu" : window.location.pathname === "/inventory" ? "inventory" : window.location.pathname === "/losses" ? "losses" : "today";
  const [workspace, setWorkspace] = useState<Workspace>(workspaceForPath);
  const apiUrl = import.meta.env.VITE_API_URL ?? "http://localhost:8080";

  useEffect(() => {
    if (!isLoading && !user && window.location.pathname === "/login") {
      const context = new URLSearchParams(window.location.search).get("context") ?? undefined;
      void signIn({ context });
    }
  }, [isLoading, signIn, user]);

  const request: ApiRequest = useCallback(async <T,>(path: string, init?: RequestInit): Promise<T> => {
    const token = await getAccessToken();
    const headers = new Headers(init?.headers);
    if (!(init?.body instanceof FormData)) headers.set("Content-Type", "application/json");
    headers.set("Authorization", `Bearer ${token}`);
    const response = await fetch(`${apiUrl}${path}`, {
      ...init,
      headers,
    });
    const body = await response.json().catch(() => null) as { error?: string; code?: string } | null;
    if (!response.ok) {
      const error = new Error(body?.error ?? "Daybook couldn't reach the kitchen. Please try again.") as Error & { status: number; code?: string };
      error.status = response.status;
      error.code = body?.code;
      throw error;
    }
    return body as T;
  }, [apiUrl, getAccessToken]);

  const loadApp = useCallback(() => {
    if (!user) return;
    setAppState({ status: "loading" });
    void request<{ restaurant: Restaurant | null }>("/v1/me")
      .then(({ restaurant }) => setAppState({ status: "ready", restaurant }))
      .catch((error: unknown) => setAppState({ status: "error", message: error instanceof Error ? error.message : "Daybook couldn't load. Please try again." }));
  }, [request, user]);

  useEffect(loadApp, [loadApp]);
  useEffect(() => {
    const onPopState = () => setWorkspace(workspaceForPath());
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);
  useEffect(() => {
    if (appState.status === "ready" && appState.restaurant?.role !== "owner" && workspace === "brief") {
      window.history.replaceState({}, "", "/today");
      setWorkspace("today");
    }
  }, [appState, workspace]);

  function openWorkspace(next: Workspace) {
    const path = `/${next}`;
    if (window.location.pathname !== path) window.history.pushState({}, "", path);
    setWorkspace(next);
  }

  function openTarget(path: string) {
    const target = path === "/invoices" ? "invoices" : path === "/menu" ? "menu" : path === "/inventory" ? "inventory" : "today";
    openWorkspace(target);
  }

  if (isLoading) {
    return <StatusPage message="Checking your session…" />;
  }

  if (!user) {
    return <Welcome authConfigured onSignIn={signIn} onSignUp={signUp} />;
  }

  if (appState.status === "loading") return <StatusPage message="Opening your daybook…" />;
  if (appState.status === "error") return <ErrorPage message={appState.message} onRetry={loadApp} onSignOut={() => signOut()} />;
  if (!appState.restaurant) {
    return <Onboarding onSignOut={() => signOut()} onCreate={(input) => request<Restaurant>("/v1/restaurants", { method: "POST", body: JSON.stringify(input) }).then((restaurant) => setAppState({ status: "ready", restaurant }))} />;
  }

  const restaurant = appState.restaurant;

  return (
    <main className="app-shell">
      <AppHeader restaurantName={restaurant.name} onSignOut={() => signOut()} />
      <nav className="workspace-nav" aria-label="Daybook sections">
        <button type="button" aria-current={workspace === "today" ? "page" : undefined} onClick={() => openWorkspace("today")}>Today</button>
        {restaurant.role === "owner" && <button type="button" aria-current={workspace === "brief" ? "page" : undefined} onClick={() => openWorkspace("brief")}>Brief</button>}
        <button type="button" aria-current={workspace === "invoices" ? "page" : undefined} onClick={() => openWorkspace("invoices")}>Invoices</button>
        <button type="button" aria-current={workspace === "sales" ? "page" : undefined} onClick={() => openWorkspace("sales")}>Sales</button>
        <button type="button" aria-current={workspace === "menu" ? "page" : undefined} onClick={() => openWorkspace("menu")}>Menu</button>
        <button type="button" aria-current={workspace === "inventory" ? "page" : undefined} onClick={() => openWorkspace("inventory")}>Inventory</button>
        <button type="button" aria-current={workspace === "losses" ? "page" : undefined} onClick={() => openWorkspace("losses")}>Losses</button>
      </nav>
      <div hidden={workspace !== "today"}><TodayWorkspace request={request} active={workspace === "today"} onNavigate={openTarget} /></div>
      {restaurant.role === "owner" && <div hidden={workspace !== "brief"}><WeeklyBriefWorkspace request={request} active={workspace === "brief"} /></div>}
      <div hidden={workspace !== "invoices"}><InvoiceWorkspace restaurant={restaurant} request={request} active={workspace === "invoices"} /></div>
      <div hidden={workspace !== "sales"}><SalesWorkspace restaurant={restaurant} request={request} active={workspace === "sales"} /></div>
      <div hidden={workspace !== "menu"}><MenuWorkspace restaurant={restaurant} request={request} active={workspace === "menu"} /></div>
      <div hidden={workspace !== "inventory"}><InventoryWorkspace restaurant={restaurant} request={request} /></div>
      <div hidden={workspace !== "losses"}><LossesWorkspace restaurant={restaurant} request={request} active={workspace === "losses"} /></div>
    </main>
  );
}

const inventoryUnits = ["each", "lb", "oz", "kg", "g", "case", "bag", "bottle", "can", "gal", "L"];
type ItemFields = { name: string; category: string; countUnit: string; parLevel: string; active: boolean };
const blankItem: ItemFields = { name: "", category: "", countUnit: "each", parLevel: "", active: true };

function InventoryWorkspace({ restaurant, request }: { restaurant: Restaurant; request: <T>(path: string, init?: RequestInit) => Promise<T> }) {
  const manager = restaurant.role === "owner" || restaurant.role === "manager";
  const [items, setItems] = useState<InventoryItem[]>([]);
  const [count, setCount] = useState<InventoryCount | null>(null);
  const [mode, setMode] = useState<"overview" | "count" | "review">("overview");
  const [quantities, setQuantities] = useState<Record<string, string>>( {} );
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [fields, setFields] = useState<ItemFields>(blankItem);
  const [editing, setEditing] = useState<InventoryItem | null>(null);

  const adoptCount = (value: InventoryCount) => {
    setCount(value);
    setQuantities(Object.fromEntries(value.entries.map(entry => [entry.id, entry.quantity ?? ""])));
  };
  const loadOverview = useCallback(async () => {
    setLoading(true); setError("");
    try {
      const [nextItems, draft] = await Promise.all([request<InventoryItem[]>("/v1/inventory-items"), request<InventoryDraftResponse>("/v1/inventory-counts/draft")]);
      setItems(nextItems); setCount(draft.count);
      if (draft.count) setQuantities(Object.fromEntries(draft.count.entries.map(entry => [entry.id, entry.quantity ?? ""])));
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Inventory couldn't load. Try again."); }
    finally { setLoading(false); }
  }, [request]);
  useEffect(() => { void loadOverview(); }, [loadOverview]);

  async function startOrResume() {
    setError(""); setNotice("");
    if (count) { setMode("count"); return; }
    setBusy(true);
    try { const value = await request<InventoryCount>("/v1/inventory-counts", { method: "POST", body: "{}" }); adoptCount(value); setMode("count"); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "The count couldn't start."); }
    finally { setBusy(false); }
  }
  function payload(active = fields.active) { return { name: fields.name, category: fields.category || null, countUnit: fields.countUnit, parLevel: fields.parLevel || null, active }; }
  async function saveItem(event: FormEvent) {
    event.preventDefault(); setError(""); setNotice("");
    if (!fields.name.trim() || !fields.countUnit.trim()) { setError("Add an item name and count unit."); return; }
    setBusy(true);
    try {
      await request(editing ? `/v1/inventory-items/${editing.id}` : "/v1/inventory-items", { method: editing ? "PUT" : "POST", body: JSON.stringify(payload()) });
      setNotice(editing ? `${fields.name.trim()} updated.` : `${fields.name.trim()} added.`); setFields(blankItem); setEditing(null); await loadOverview();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "The item couldn't be saved."); }
    finally { setBusy(false); }
  }
  function edit(item: InventoryItem) { setEditing(item); setFields({ name:item.name, category:item.category ?? "", countUnit:item.countUnit, parLevel:item.parLevel ?? "", active:item.active }); window.scrollTo({ top: 0, behavior: "smooth" }); }
  async function toggle(item: InventoryItem) {
    setBusy(true); setError("");
    try { await request(`/v1/inventory-items/${item.id}`, { method:"PUT", body:JSON.stringify({ name:item.name, category:item.category, countUnit:item.countUnit, parLevel:item.parLevel, active:!item.active }) }); setNotice(`${item.name} ${item.active ? "archived" : "reactivated"}.`); await loadOverview(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "The item couldn't be updated."); }
    finally { setBusy(false); }
  }
  async function saveDraft(showNotice = true) {
    if (!count) return null; setBusy(true); setError(""); if (showNotice) setNotice("");
    try { const value = await request<InventoryCount>(`/v1/inventory-counts/${count.id}`, { method:"PUT", body:JSON.stringify({ revision:count.revision, entries:count.entries.map(entry => ({ id:entry.id, quantity:quantities[entry.id]?.trim() || null })) }) }); adoptCount(value); if (showNotice) setNotice("Draft saved."); return value; }
    catch (reason) { setError(reason instanceof Error ? reason.message : "The draft couldn't be saved. Check the quantities and try again."); return null; }
    finally { setBusy(false); }
  }
  async function reviewCount() { const saved = await saveDraft(false); if (saved) { setNotice(""); setMode("review"); } }
  async function backToOverview() { const saved = await saveDraft(false); if (saved) { setMode("overview"); setNotice("Draft saved. Resume when you're ready."); } }
  async function complete() {
    if (!count) return; const missing = count.entries.filter(entry => !quantities[entry.id]?.trim()); setBusy(true); setError("");
    try { await request(`/v1/inventory-counts/${count.id}/complete`, { method:"POST", body:JSON.stringify({ confirmMissing:missing.length > 0, revision:count.revision }) }); setCount(null); setQuantities({}); setMode("overview"); setNotice("Inventory count completed."); await loadOverview(); setNotice("Inventory count completed."); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "The count couldn't be completed."); }
    finally { setBusy(false); }
  }

  if (mode !== "overview" && count) {
    const groups = groupByCategory(count.entries); const missing = count.entries.filter(entry => !quantities[entry.id]?.trim());
    if (mode === "review") return <section className="inventory-workspace count-workspace"><button className="text-button" type="button" onClick={() => setMode("count")}>← Back to count</button><header className="inventory-heading"><p className="section-code">DB—INVENTORY / REVIEW</p><h1>Review count</h1><p>{count.entries.length - missing.length} counted · {missing.length} missing</p></header>{missing.length > 0 && <div className="missing-list"><h2>Missing quantities</h2><p>These items will stay blank in this count.</p><ul>{missing.map(entry => <li key={entry.id}><strong>{entry.name}</strong> · {entry.countUnit}</li>)}</ul></div>}{error && <p className="form-error" role="alert">{error}</p>}<div className="count-actions"><button className="file-button" type="button" onClick={() => setMode("count")}>Back to count</button><button className="ledger-button" type="button" disabled={busy} onClick={() => void complete()}>{busy ? "Completing…" : missing.length ? "Complete with missing items" : "Complete count"}</button></div></section>;
    return <section className="inventory-workspace count-workspace"><button className="text-button" type="button" disabled={busy} onClick={() => void backToOverview()}>← Save and return to overview</button><header className="inventory-heading"><p className="section-code">DB—INVENTORY / COUNT</p><h1>Count what is on hand</h1><p>Your saved draft stays here when you return to the overview.</p></header>{groups.map(([category, entries]) => <section className="count-category" key={category}><h2>{category}</h2>{entries.map(entry => <label className="count-row" key={entry.id}><span><strong>{entry.name}</strong><small>Count in {entry.countUnit}</small></span><span className="quantity-field"><input aria-label={`${entry.name}, quantity in ${entry.countUnit}`} inputMode="decimal" value={quantities[entry.id] ?? ""} onChange={event => setQuantities(current => ({...current,[entry.id]:event.target.value}))}/><b>{entry.countUnit}</b></span></label>)}</section>)}{error && <p className="form-error" role="alert">{error}</p>}{notice && <p className="success-notice" role="status">{notice}</p>}<div className="count-actions"><button className="file-button" type="button" disabled={busy} onClick={() => void saveDraft()}>{busy ? "Saving…" : "Save draft"}</button><button className="ledger-button" type="button" disabled={busy} onClick={() => void reviewCount()}>Review count</button></div></section>;
  }

  const active = items.filter(item => item.active); const archived = items.filter(item => !item.active);
  return <section className="inventory-workspace"><header className="inventory-heading"><p className="section-code">DB—INVENTORY</p><h1>{restaurant.name} inventory</h1><p>See what is on hand and keep the next count moving.</p><button className="ledger-button" type="button" disabled={busy || (!count && active.length === 0)} onClick={() => void startOrResume()}>{busy ? "Opening…" : count ? "Resume count" : "Start count"}</button></header>
    {error && <p className="form-error inventory-message" role="alert">{error}</p>}{notice && <p className="success-notice inventory-message" role="status">{notice}</p>}
    {manager && <form className="inventory-item-form" onSubmit={saveItem}><div className="list-heading"><h2>{editing ? "Edit item" : "Add an item"}</h2>{editing && <button className="text-button" type="button" onClick={() => {setEditing(null);setFields(blankItem)}}>Cancel</button>}</div><div className="inventory-form-fields"><label>Name<input required maxLength={50} value={fields.name} onChange={e=>setFields({...fields,name:e.target.value})}/></label><label>Category <span>Optional</span><input maxLength={20} value={fields.category} onChange={e=>setFields({...fields,category:e.target.value})}/></label><label>Count unit<select required value={fields.countUnit} onChange={e=>setFields({...fields,countUnit:e.target.value})}>{inventoryUnits.map(unit=><option key={unit} value={unit}>{unit}</option>)}</select></label><label>Par level <span>Optional</span><input inputMode="decimal" value={fields.parLevel} onChange={e=>setFields({...fields,parLevel:e.target.value})}/></label></div>{editing && <label className="active-toggle"><input type="checkbox" checked={fields.active} onChange={e=>setFields({...fields,active:e.target.checked})}/> Active item</label>}<button className="ledger-button" disabled={busy}>{busy ? "Saving…" : editing ? "Save item" : "Add item"}</button></form>}
    <div className="inventory-list"><div className="list-heading"><h2>Active items</h2><button className="text-button" type="button" onClick={() => void loadOverview()}>Refresh</button></div>{loading ? <p role="status">Loading inventory…</p> : active.length === 0 ? <p className="empty-state">{manager ? "No active items yet. Add your first item above, including how the crew counts it." : "No inventory items are ready to count. Ask an owner or manager to add them."}</p> : groupByCategory(active).map(([category, group]) => <InventoryCategory key={category} category={category} items={group} manager={manager} busy={busy} onEdit={edit} onToggle={toggle}/>)}</div>
    {!loading && archived.length > 0 && <section className="archived-section"><h2>Archived items</h2><p>Kept here with their count history.</p>{groupByCategory(archived).map(([category, group])=><InventoryCategory key={category} category={category} items={group} manager={manager} busy={busy} onEdit={edit} onToggle={toggle}/>)}</section>}
  </section>;
}

function groupByCategory<T extends {category:string|null}>(values:T[]): [string,T[]][] { const groups = new Map<string,T[]>(); values.forEach(value => { const key=value.category?.trim() || "Uncategorized"; groups.set(key,[...(groups.get(key)??[]),value]); }); return [...groups.entries()]; }
function InventoryCategory({category,items,manager,busy,onEdit,onToggle}:{category:string;items:InventoryItem[];manager:boolean;busy:boolean;onEdit:(item:InventoryItem)=>void;onToggle:(item:InventoryItem)=>void}) { return <section className="inventory-category"><h3>{category}</h3><div className="inventory-cards">{items.map(item=><article className={`inventory-card${item.lowStock?" low-stock":""}`} key={item.id}><div className="inventory-card-head"><div><h4>{item.name}</h4>{item.lowStock&&<strong className="low-stock-label">Low stock</strong>}</div><p className="current-quantity">{item.latestQuantity===null?"Not counted":`${formatInventoryNumber(item.latestQuantity)} ${item.countUnit}`}</p></div><div className="inventory-metrics"><p><span>Previous</span>{item.previousQuantity===null?"—":`${formatInventoryNumber(item.previousQuantity)} ${item.countUnit}`}</p><p><span>Change</span>{item.change===null?"—":`${formatSigned(item.change)} ${item.countUnit}`}</p><p><span>Last counted</span>{item.lastCountedAt?formatInventoryDate(item.lastCountedAt):"Not yet"}</p></div>{manager&&<div className="card-actions"><button className="file-button" type="button" disabled={busy} onClick={()=>onEdit(item)}>Edit</button><button className="text-button" type="button" disabled={busy} onClick={()=>void onToggle(item)}>{item.active?"Archive":"Reactivate"}</button></div>}</article>)}</div></section> }
function formatInventoryNumber(value:string) { const number=Number(value); return Number.isFinite(number) ? new Intl.NumberFormat(undefined,{maximumFractionDigits:6}).format(number) : value; }
function formatSigned(value:string) { const number=Number(value); if (!Number.isFinite(number)) return value; return `${number>0?"+":""}${formatInventoryNumber(value)}`; }
function formatInventoryDate(value:string) { const date=new Date(value); return Number.isNaN(date.getTime())?value:new Intl.DateTimeFormat(undefined,{dateStyle:"medium",timeStyle:"short"}).format(date); }

const wasteReasons = [
  { value: "spoilage", label: "Spoilage" },
  { value: "overproduction", label: "Overproduction" },
  { value: "prep_mistake", label: "Prep mistake" },
  { value: "portioning", label: "Portioning" },
  { value: "dropped_damaged", label: "Dropped or damaged" },
  { value: "returned", label: "Returned" },
  { value: "expired", label: "Expired" },
  { value: "other", label: "Other" },
];
const stockoutReasons = [
  { value: "delivery_late_or_missed", label: "Delivery late or missed" },
  { value: "ordered_too_little", label: "Ordered too little" },
  { value: "demand_higher_than_expected", label: "Demand higher than expected" },
  { value: "prep_or_portion_issue", label: "Prep or portion issue" },
  { value: "waste_or_spoilage", label: "Waste or spoilage" },
  { value: "other", label: "Other" },
];
const stockoutSeverities = [
  { value: "some_orders", label: "Some orders affected" },
  { value: "menu_item_unavailable", label: "Menu item unavailable" },
  { value: "service_blocker", label: "Service blocker" },
];

function LossesWorkspace({ restaurant, request, active }: { restaurant: Restaurant; request: <T>(path: string, init?: RequestInit) => Promise<T>; active: boolean }) {
  const manager = restaurant.role === "owner" || restaurant.role === "manager";
  const [items, setItems] = useState<InventoryItem[]>([]);
  const [logs, setLogs] = useState<LossEvent[]>([]);
  const [itemsLoading, setItemsLoading] = useState(true);
  const [logsLoading, setLogsLoading] = useState(true);
  const [itemsError, setItemsError] = useState("");
  const [logsError, setLogsError] = useState("");
  const [eventType, setEventType] = useState<LossEventType>("waste");
  const [inventoryItemId, setInventoryItemId] = useState("");
  const [quantity, setQuantity] = useState("");
  const [severity, setSeverity] = useState("");
  const [reason, setReason] = useState("");
  const [note, setNote] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const loadItems = useCallback(() => {
    setItemsLoading(true);
    setItemsError("");
    void request<InventoryItem[]>("/v1/inventory-items")
      .then(values => setItems(values.filter(item => item.active)))
      .catch((cause: unknown) => setItemsError(cause instanceof Error ? cause.message : "Inventory items couldn't load. Try again."))
      .finally(() => setItemsLoading(false));
  }, [request]);
  const loadLogs = useCallback(() => {
    setLogsLoading(true);
    setLogsError("");
    void request<LossEvent[]>("/v1/loss-events")
      .then(values => setLogs(current => mergeLossLogs([...current, ...values])))
      .catch((cause: unknown) => setLogsError(cause instanceof Error ? cause.message : "Recent logs couldn't load. Try again."))
      .finally(() => setLogsLoading(false));
  }, [request]);

  useEffect(() => {
    if (!active) return;
    loadItems();
    loadLogs();
  }, [active, loadItems, loadLogs]);

  function chooseType(next: LossEventType) {
    setEventType(next);
    setQuantity("");
    setSeverity("");
    setReason("");
    setError("");
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError("");
    setNotice("");
    if (!inventoryItemId) { setError("Choose the inventory item this log is about."); return; }
    if (eventType === "waste" && (!/^\d+(?:\.\d{1,6})?$/.test(quantity) || /^0+(?:\.0+)?$/.test(quantity))) {
      setError("Enter a positive waste quantity with up to 6 decimal places.");
      return;
    }
    if (eventType === "stockout" && !severity) { setError("Choose how the stockout affected service."); return; }
    if (!reason) { setError(`Choose a ${eventType} reason.`); return; }
    if (note.trim().length > 500) { setError("Keep the note to 500 characters or fewer."); return; }
    setSaving(true);
    try {
      const created = await request<LossEvent>("/v1/loss-events", {
        method: "POST",
        body: JSON.stringify({
          eventType,
          inventoryItemId,
          quantity: eventType === "waste" ? quantity : null,
          severity: eventType === "stockout" ? severity : null,
          reason,
          note: note.trim() || null,
        }),
      });
      setLogs(current => mergeLossLogs([created, ...current]));
      setLogsError("");
      setInventoryItemId("");
      setQuantity("");
      setSeverity("");
      setReason("");
      setNote("");
      setNotice(eventType === "waste" ? "Waste logged. Your last count stays unchanged." : "Stockout logged. Your inventory count stays unchanged.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : `The ${eventType} couldn't be logged. Try again.`);
    } finally {
      setSaving(false);
    }
  }

  const selectedItem = items.find(item => item.id === inventoryItemId);
  const reasons = eventType === "waste" ? wasteReasons : stockoutReasons;
  return <section className="losses-workspace" aria-labelledby="losses-heading">
    <header className="inventory-heading"><p className="section-code">DB—LOSSES</p><h1 id="losses-heading">Waste &amp; stockouts</h1><p>Record what happened without changing the last inventory count.</p></header>
    {itemsLoading ? <p className="loss-loading" role="status">Loading active inventory items…</p> : itemsError ? <div className="loss-load-error"><p className="form-error" role="alert">{itemsError}</p><button className="file-button" type="button" onClick={loadItems}>Retry inventory</button></div> : items.length === 0 ? <div className="loss-no-items"><h2>No active items to log</h2><p>{manager ? "Add or reactivate an item in Inventory before logging waste or a stockout." : "Ask an owner or manager to add an active inventory item first."}</p></div> : <form className="loss-form" onSubmit={submit}>
      <p className="loss-ready"><span aria-hidden="true">●</span> Ready to log an event</p>
      <fieldset className="loss-type-fieldset"><legend>What happened?</legend><div className="loss-type-grid">
        <label className={`loss-type-card${eventType === "waste" ? " selected" : ""}`}><input type="radio" name="loss-type" value="waste" checked={eventType === "waste"} onChange={() => chooseType("waste")}/><span className="loss-type-icon" aria-hidden="true">↓</span><span><strong>Waste</strong><small>Something was discarded or could not be used.</small></span></label>
        <label className={`loss-type-card${eventType === "stockout" ? " selected" : ""}`}><input type="radio" name="loss-type" value="stockout" checked={eventType === "stockout"} onChange={() => chooseType("stockout")}/><span className="loss-type-icon" aria-hidden="true">!</span><span><strong>Stockout</strong><small>An unavailable item affected orders or service.</small></span></label>
      </div></fieldset>
      <label className="loss-field">Inventory item<select required value={inventoryItemId} onChange={event => { setInventoryItemId(event.target.value); setError(""); }}><option value="">Choose an active item</option>{items.map(item => <option key={item.id} value={item.id}>{item.name} · {item.category || "Uncategorized"} · {item.countUnit}</option>)}</select>{selectedItem && <small>{selectedItem.category || "Uncategorized"} · Counted in {selectedItem.countUnit}</small>}</label>
      {eventType === "waste" ? <label className="loss-field">Quantity discarded<span className="quantity-field loss-quantity"><input required aria-describedby="loss-quantity-help" inputMode="decimal" placeholder="0" value={quantity} onChange={event => { setQuantity(event.target.value); setError(""); }}/><b>{selectedItem?.countUnit || "unit"}</b></span><small id="loss-quantity-help">Use the item's fixed count unit. This does not change the last count.</small></label> : <fieldset className="loss-option-fieldset"><legend>Service impact</legend><div className="loss-chip-grid">{stockoutSeverities.map(option => <label className={`loss-option${severity === option.value ? " selected" : ""}`} key={option.value}><input type="radio" name="severity" value={option.value} checked={severity === option.value} onChange={() => { setSeverity(option.value); setError(""); }}/><span>{option.label}</span></label>)}</div></fieldset>}
      <fieldset className="loss-option-fieldset"><legend>Reason</legend><div className="loss-chip-grid">{reasons.map(option => <label className={`loss-option${reason === option.value ? " selected" : ""}`} key={option.value}><input type="radio" name="reason" value={option.value} checked={reason === option.value} onChange={() => { setReason(option.value); setError(""); }}/><span>{option.label}</span></label>)}</div></fieldset>
      <label className="loss-field">Note <span>Optional</span><textarea maxLength={500} rows={3} value={note} onChange={event => setNote(event.target.value)} placeholder="Add useful shift context"/><small>{note.length}/500</small></label>
      {error && <p className="form-error" role="alert">{error}</p>}
      {notice && <p className="success-notice" role="status">{notice}</p>}
      <div className="loss-submit"><button className="ledger-button" disabled={saving}>{saving ? "Saving…" : eventType === "waste" ? "Log waste" : "Log stockout"}</button></div>
    </form>}
    <section className="recent-losses" aria-labelledby="recent-losses-heading"><div className="list-heading"><h2 id="recent-losses-heading">Recent logs</h2><button className="text-button" type="button" disabled={logsLoading} onClick={loadLogs}>Refresh</button></div>
      {logsLoading ? <p className="empty-state" role="status">Loading recent waste and stockouts…</p> : logsError ? <div className="loss-load-error"><p className="form-error" role="alert">{logsError}</p><button className="file-button" type="button" onClick={loadLogs}>Retry recent logs</button></div> : logs.length === 0 ? <p className="empty-state">No waste or stockouts logged yet. New logs will appear here.</p> : <div className="recent-loss-list">{logs.map(log => <article className={`recent-loss-card ${log.eventType}`} key={log.id}><div><p className="invoice-status">{log.eventType === "waste" ? "Waste" : "Stockout"}</p><h3>{log.inventoryItemName}</h3><p className="loss-result">{log.eventType === "waste" ? `${log.quantity} ${log.countUnit}` : lossOptionLabel(stockoutSeverities, log.severity)}</p></div><div className="loss-log-detail"><p><span>Reason</span>{lossOptionLabel(log.eventType === "waste" ? wasteReasons : stockoutReasons, log.reason)}</p>{log.note && <p className="loss-note">{log.note}</p>}<time dateTime={log.createdAt}>{formatInventoryDate(log.createdAt)}</time></div></article>)}</div>}
    </section>
  </section>;
}

function lossOptionLabel(options: { value: string; label: string }[], value: string | null) { return options.find(option => option.value === value)?.label ?? value ?? "—"; }
function mergeLossLogs(values: LossEvent[]) { const byId = new Map(values.map(value => [value.id, value])); return [...byId.values()].sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.id.localeCompare(a.id)).slice(0, 50); }

function MenuWorkspace({ restaurant, request, active }: { restaurant: Restaurant; request: <T>(path: string, init?: RequestInit) => Promise<T>; active: boolean }) {
  const owner = restaurant.role === "owner";
  const canManageCosting = owner || restaurant.role === "manager";
  const [items, setItems] = useState<MenuItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [name, setName] = useState("");
  const [category, setCategory] = useState("");
  const [price, setPrice] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState("");
  const [imports, setImports] = useState<MenuImport[]>([]);
  const [importFile, setImportFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);
  const [review, setReview] = useState<{ import: MenuImport; items: MenuImportItem[] } | null>(null);
  const [retryingId, setRetryingId] = useState("");
  const [openingId, setOpeningId] = useState("");
  const [costingItem, setCostingItem] = useState<MenuItem | null>(null);

  const loadMenu = useCallback(() => {
    setLoading(true);
    setError("");
    void request<MenuItem[]>("/v1/menu-items")
      .then(setItems)
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Your menu couldn't load."))
      .finally(() => setLoading(false));
  }, [request]);

  const loadImports = useCallback(() => {
    void request<MenuImport[]>("/v1/menu-imports")
      .then(setImports)
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : "Menu imports couldn't load."));
  }, [request]);

  useEffect(() => { if (active) loadMenu(); }, [active, loadMenu]);
  useEffect(() => { if (active && owner) loadImports(); }, [active, loadImports, owner]);
  useEffect(() => {
    if (!active || !owner || !imports.some((menuImport) => menuImport.status === "processing")) return;
    const timer = window.setTimeout(loadImports, 15000);
    return () => window.clearTimeout(timer);
  }, [active, imports, loadImports, owner]);
  async function submit(event:FormEvent<HTMLFormElement>){event.preventDefault();setError("");setNotice("");if(!name.trim()||!price.trim()){setError("Add the menu item name and selling price.");return}setSaving(true);try{const item=await request<MenuItem>("/v1/menu-items",{method:"POST",body:JSON.stringify({name,category:category||null,sellingPrice:price,currency})});setItems((current)=>[...current,item].sort((a,b)=>(a.category??"zzz").localeCompare(b.category??"zzz")||a.name.localeCompare(b.name)));setName("");setCategory("");setPrice("");setNotice(`${item.name} added to the menu.`);}catch(reason){setError(reason instanceof Error?reason.message:"The menu item couldn't be saved.");}finally{setSaving(false)}}
  async function uploadMenu(e:FormEvent<HTMLFormElement>){e.preventDefault();setError("");setNotice("");if(!importFile){setError("Choose one menu photo or PDF.");return}if(importFile.size>10*1024*1024){setError("Choose a file smaller than 10 MiB.");return}const body=new FormData();body.append("file",importFile);setUploading(true);try{const value=await request<MenuImport>("/v1/menu-imports",{method:"POST",body});setImports(v=>[value,...v]);setImportFile(null);setNotice("Menu uploaded. Extraction is processing.");(e.currentTarget as HTMLFormElement).reset()}catch(reason){setError(reason instanceof Error?reason.message:"Menu upload failed. Try again.")}finally{setUploading(false)}}
  async function openReview(id:string){setError("");try{const value=await request<{import:MenuImport;items:MenuImportItem[]}>(`/v1/menu-imports/${id}`);setReview({...value,items:value.items.map(v=>({...v,selected:isValidMenuImportItem(v)}))})}catch(reason){setError(reason instanceof Error?reason.message:"Review couldn't open.")}}
  async function retryMenu(menuImport:MenuImport){setRetryingId(menuImport.id);setError("");try{await request(`/v1/menu-imports/${menuImport.id}/retry`,{method:"POST"});loadImports()}catch(reason){setError(reason instanceof Error?reason.message:"The menu couldn't be retried.")}finally{setRetryingId("")}}
  async function openOriginal(menuImport:MenuImport){setOpeningId(menuImport.id);setError("");const popup=window.open("","_blank");if(popup)popup.opener=null;try{const {url}=await request<{url:string}>(`/v1/menu-imports/${menuImport.id}/file`);if(popup)popup.location.href=url;else window.open(url,"_blank","noopener,noreferrer")}catch(reason){popup?.close();setError(reason instanceof Error?reason.message:"The original couldn't open. Please try again.")}finally{setOpeningId("")}}
  async function approve(){if(!review)return;setError("");const selected=review.items.filter(v=>v.selected);if(!selected.length){setError("Select at least one valid item.");return}if(selected.some(v=>!isValidMenuImportItem(v))){setError("Check each selected item's name, positive price, and three-letter currency.");return}setSaving(true);try{const counts=await request<{imported:number;skipped:number}>(`/v1/menu-imports/${review.import.id}`,{method:"PUT",body:JSON.stringify({items:selected.map(({id,name,category,sellingPrice,currency})=>({id,name,category,sellingPrice,currency}))})});setNotice(`${counts.imported} items imported${counts.skipped?`; ${counts.skipped} duplicates skipped`:""}.`);setReview(null);loadMenu();loadImports()}catch(reason){setError(reason instanceof Error?reason.message:"Items couldn't be imported.")}finally{setSaving(false)}}
  if(costingItem)return <MenuItemCosting item={costingItem} request={request} onBack={()=>{setCostingItem(null);loadMenu()}} onSaved={ingredientCount=>{setItems(current=>current.map(item=>item.id===costingItem.id?{...item,ingredientCount}:item));setCostingItem(current=>current?{...current,ingredientCount}:current)}}/>;
  if(review)return <section className="review-shell"><button className="text-button" type="button" onClick={()=>{setError("");setReview(null)}}>← Back to menu</button><h1>Review menu</h1><p>Edit and select items with a clear selling price. Nothing is added until you import.</p><div className="review-form">{review.items.map((row,index)=>{const needsAttention=row.hasWarnings||!isValidMenuImportItem(row);return <article className={`line-card${needsAttention?" needs-attention":""}`} key={row.id??`new-${index}`}>{needsAttention&&<p className="review-warning" role="status">Check this item against the original menu.</p>}<label><input type="checkbox" checked={Boolean(row.selected)} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,selected:e.target.checked}:x)}))}/> Import this item</label><label>Name<input value={row.name} maxLength={50} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,name:e.target.value}:x)}))}/></label><label>Category<input value={row.category??""} maxLength={20} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,category:e.target.value||null}:x)}))}/></label><label>Price<input inputMode="decimal" value={row.sellingPrice??""} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,sellingPrice:e.target.value||null}:x)}))}/></label><label>Currency<input maxLength={3} value={row.currency??""} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,currency:e.target.value.toUpperCase()||null}:x)}))}/></label><button className="text-button" type="button" onClick={()=>setReview(v=>v&&({...v,items:v.items.filter((_,i)=>i!==index)}))}>Remove row</button></article>})}<button className="file-button" type="button" onClick={()=>setReview(v=>v&&({...v,items:[...v.items,{name:"",category:null,sellingPrice:null,currency:"USD",hasWarnings:true,selected:true}]}))}>Add item</button>{error&&<p className="form-error" role="alert">{error}</p>}<button className="ledger-button" type="button" disabled={saving} onClick={approve}>{saving?"Importing…":"Import selected items"}</button></div></section>;
  return <section className="menu-workspace" aria-labelledby="menu-heading">
    <header className="invoice-heading">
      <p className="section-code">DB—MENU / TOP ITEMS</p>
      <h1 id="menu-heading">{restaurant.name} menu</h1>
      <p>{owner ? "Add items by hand or review one menu photo or PDF." : canManageCosting ? "Review menu items and set up current approximate ingredient costs." : "Review the restaurant's active menu items."}</p>
    </header>
    {error&&<p className="form-error menu-message" role="alert">{error}</p>}
    {notice&&<p className="success-notice menu-message" role="status">{notice}</p>}
    <div className={`menu-grid${owner?"":" menu-grid-read-only"}`}>
      {owner&&<div>
        <form className="invoice-form menu-form" onSubmit={submit} noValidate>
          <h2>Add a menu item</h2><p>Record the name customers see and its current selling price.</p>
          <div className="ledger-field"><label htmlFor="menu-name">Menu item</label><input id="menu-name" value={name} onChange={e=>setName(e.target.value)} maxLength={50} required/></div>
          <div className="ledger-field"><label htmlFor="menu-category">Category <span className="optional-label">Optional</span></label><input id="menu-category" value={category} onChange={e=>setCategory(e.target.value)} maxLength={20}/></div>
          <div className="menu-price-fields"><div className="ledger-field"><label htmlFor="menu-price">Selling price</label><input id="menu-price" inputMode="decimal" value={price} onChange={e=>setPrice(e.target.value)} required/></div><div className="ledger-field"><label htmlFor="menu-currency">Currency</label><select id="menu-currency" value={currency} onChange={e=>setCurrency(e.target.value)}><option>USD</option><option>CAD</option><option>GBP</option><option>EUR</option></select></div></div>
          <button className="ledger-button" disabled={saving}>{saving?"Adding item…":"Add to menu"}</button>
        </form>
        <form className="invoice-form menu-import-form" onSubmit={uploadMenu}>
          <h2>Import from photo</h2><p>Upload one PDF or photo. You'll review every item before it is added.</p>
          <label className="file-button" htmlFor="menu-file">Choose photo or PDF</label><input className="visually-hidden" id="menu-file" type="file" accept="application/pdf,image/jpeg,image/png,image/webp" onChange={e=>setImportFile(e.target.files?.[0]??null)}/>
          {importFile&&<span className="selected-file">{importFile.name}</span>}
          <button className="ledger-button" disabled={uploading}>{uploading?"Uploading…":"Upload menu"}</button>
        </form>
      </div>}
      <div className="menu-list">
        <div className="list-heading"><h2>Active menu items</h2><button className="text-button" type="button" onClick={loadMenu}>Refresh</button></div>
        {loading?<p role="status">Loading menu…</p>:items.length===0?<p className="empty-state">No menu items yet.</p>:<div className="menu-cards">{items.map(item=><article className="menu-card" key={item.id}><div className="menu-card-main"><div><p className="invoice-status">{item.category??"Uncategorized"}</p><h3>{item.name}</h3></div><strong>{formatMoney(item.sellingPrice,item.currency)}</strong></div>{canManageCosting&&<button className="file-button costing-card-action" type="button" onClick={()=>setCostingItem(item)}>{item.ingredientCount>0?"Review ingredient cost":"Set up ingredient cost"}</button>}</article>)}</div>}
        {owner&&<><div className="list-heading import-heading"><h2>Menu imports</h2></div>{imports.length===0?<p className="empty-state">No menu imports yet.</p>:<div className="menu-cards">{imports.map(value=><article className="menu-card" key={value.id}><div className="menu-card-copy"><p className="invoice-status">{importStatusLabel(value.status, value.delayed, "menu")}</p><h3>{value.originalFilename}</h3></div><div className="card-actions">{value.status==="needs_review"&&<button className="file-button" type="button" onClick={()=>void openReview(value.id)}>Review menu</button>}{value.status==="failed"&&<button className="file-button" type="button" disabled={retryingId===value.id} onClick={()=>void retryMenu(value)}>{retryingId===value.id?"Trying again…":"Retry"}</button>}<button className="text-button" type="button" disabled={openingId===value.id} onClick={()=>void openOriginal(value)}>{openingId===value.id?"Opening…":"Original"}</button></div></article>)}</div>}</>}
      </div>
    </div>
  </section>;
}

function MenuItemCosting({ item, request, onBack, onSaved }: { item: MenuItem; request: <T>(path: string, init?: RequestInit) => Promise<T>; onBack: () => void; onSaved: (ingredientCount: number) => void }) {
  const [response, setResponse] = useState<CostingResponse | null>(null);
  const [drafts, setDrafts] = useState<IngredientDraft[]>([]);
  const [mode, setMode] = useState<"result" | "edit">(item.ingredientCount > 0 ? "result" : "edit");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const value = await request<CostingResponse>(`/v1/menu-items/${item.id}/costing`);
      setResponse(value);
      setDrafts(costingDrafts(value));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Ingredient cost couldn't load. Try again.");
    } finally {
      setLoading(false);
    }
  }, [item.id, request]);

  useEffect(() => { void load(); }, [load]);

  function updateDraft(index: number, patch: Partial<IngredientDraft>) {
    setDrafts(current => current.map((draft, draftIndex) => draftIndex === index ? { ...draft, ...patch } : draft));
    setNotice("");
  }

  function addIngredient() {
    if (!response || drafts.length >= 30) return;
    const used = new Set(drafts.map(draft => draft.inventoryItemId));
    const choice = response.inventoryItems.find(candidate => !used.has(candidate.id));
    if (!choice) return;
    const matchingUnit = servingUnits.find(unit => unit.value === choice.countUnit)?.value ?? "each";
    setDrafts(current => [...current, { inventoryItemId: choice.id, quantity: "", unit: matchingUnit, archived: false, inventoryItemName: choice.name }]);
  }

  async function saveIngredients(nextDrafts: IngredientDraft[], successMessage: string) {
    setError("");
    setNotice("");
    if (nextDrafts.some(draft => !isPositiveIngredientQuantity(draft.quantity))) {
      setError("Each amount must be a positive plain decimal with no more than 6 decimal places.");
      return;
    }
    if (new Set(nextDrafts.map(draft => draft.inventoryItemId)).size !== nextDrafts.length) {
      setError("Choose each inventory item only once.");
      return;
    }
    setSaving(true);
    try {
      const value = await request<CostingResponse>(`/v1/menu-items/${item.id}/ingredients`, {
        method: "PUT",
        body: JSON.stringify({ ingredients: nextDrafts.map(draft => ({ inventoryItemId: draft.inventoryItemId, quantity: draft.quantity.trim(), unit: draft.unit })) }),
      });
      setResponse(value);
      setDrafts(costingDrafts(value));
      setMode("result");
      setNotice(successMessage);
      onSaved(value.ingredients.length);
      window.scrollTo({ top: 0, behavior: "smooth" });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Ingredient setup couldn't be saved. Check each row and try again.");
    } finally {
      setSaving(false);
    }
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void saveIngredients(drafts, "Ingredient setup saved. Costs were refreshed from the latest recorded purchases.");
  }

  function stopTracking() {
    if (!window.confirm(`Stop tracking ingredient cost for ${item.name}? This removes its current ingredient setup.`)) return;
    void saveIngredients([], "Ingredient tracking stopped.");
  }

  function editSetup() {
    if (response) setDrafts(costingDrafts(response));
    setError("");
    setNotice("");
    setMode("edit");
    window.scrollTo({ top: 0, behavior: "smooth" });
  }

  function leaveEditor() {
    setError("");
    setNotice("");
    if (response?.ingredients.length) {
      setDrafts(costingDrafts(response));
      setMode("result");
    } else {
      onBack();
    }
  }

  if (loading) return <section className="review-shell costing-shell"><button className="text-button" type="button" onClick={onBack}>← Back to menu</button><p role="status">Loading current ingredient cost…</p></section>;
  if (!response) return <section className="review-shell costing-shell"><button className="text-button" type="button" onClick={onBack}>← Back to menu</button><div className="costing-load-error"><p className="form-error" role="alert">{error || "Ingredient cost couldn't load."}</p><button className="file-button" type="button" onClick={() => void load()}>Try again</button></div></section>;

  if (mode === "edit") {
    const used = new Set(drafts.map(draft => draft.inventoryItemId));
    const canAdd = drafts.length < 30 && response.inventoryItems.some(candidate => !used.has(candidate.id));
    return <section className="review-shell costing-shell" aria-labelledby="ingredient-editor-heading">
      <button className="text-button" type="button" disabled={saving} onClick={leaveEditor}>← {response.ingredients.length ? "Back to ingredient cost" : "Back to menu"}</button>
      <p className="section-code">DB—MENU / INGREDIENT SETUP</p>
      <h1 id="ingredient-editor-heading">{response.menuItem.name}</h1>
      <p>Choose the inventory items and exact amount used for one serving. Package labels such as case, bag, bottle, and can are not serving units.</p>
      <form className="ingredient-form" onSubmit={submit} noValidate>
        {drafts.length === 0 ? <div className="costing-empty"><h2>No ingredients selected</h2><p>Add only the ingredients you want to cost now. Saving an empty setup stops tracking this menu item.</p></div> : <div className="ingredient-editor-list">{drafts.map((draft, index) => {
          const currentChoice = response.inventoryItems.find(choice => choice.id === draft.inventoryItemId);
          const choices = response.inventoryItems.filter(choice => choice.id === draft.inventoryItemId || !used.has(choice.id));
          const warningId = `ingredient-archived-${index}`;
          return <fieldset className="ingredient-editor-row" key={`${draft.inventoryItemId}-${index}`}>
            <legend>Ingredient {index + 1}</legend>
            {draft.archived && <p className="review-warning" id={warningId} role="status">Archived inventory item. It remains in this setup, but it cannot be added to another menu item unless reactivated.</p>}
            {draft.archived ? <div className="archived-ingredient-name" aria-describedby={warningId}><span>Inventory item</span><strong>{draft.inventoryItemName}</strong></div> : <label>Inventory item<select value={draft.inventoryItemId} onChange={event => { const choice = response.inventoryItems.find(candidate => candidate.id === event.target.value); updateDraft(index, { inventoryItemId: event.target.value, inventoryItemName: choice?.name ?? "", unit: servingUnits.find(unit => unit.value === choice?.countUnit)?.value ?? draft.unit }); }}>{choices.map(choice => <option key={choice.id} value={choice.id}>{choice.name}{choice.category ? ` · ${choice.category}` : ""}</option>)}</select><small>Inventory count unit: {currentChoice?.countUnit ?? "Archived"}</small></label>}
            <div className="ingredient-amount-fields"><label>Amount per serving<input inputMode="decimal" placeholder="0" value={draft.quantity} aria-describedby={draft.archived ? warningId : undefined} onChange={event => updateDraft(index, { quantity: event.target.value })}/></label><label>Serving unit<select value={draft.unit} aria-describedby={draft.archived ? warningId : undefined} onChange={event => updateDraft(index, { unit: event.target.value as ServingUnit })}>{servingUnits.map(unit => <option key={unit.value} value={unit.value}>{unit.label}</option>)}</select></label></div>
            <button className="text-button" type="button" disabled={saving} onClick={() => setDrafts(current => current.filter((_, draftIndex) => draftIndex !== index))}>Remove ingredient</button>
          </fieldset>;
        })}</div>}
        {response.inventoryItems.length === 0 && <p className="empty-state">No active inventory items are available. Add or reactivate an item in Inventory first.</p>}
        <button className="file-button add-ingredient-button" type="button" disabled={!canAdd || saving} onClick={addIngredient}>{drafts.length >= 30 ? "30 ingredient limit reached" : canAdd ? "Add ingredient" : "No more active items"}</button>
        {error && <p className="form-error" role="alert">{error}</p>}
        <div className="ingredient-save-actions"><p>Saving replaces this menu item's full current ingredient setup.</p><button className="ledger-button" type="submit" disabled={saving}>{saving ? "Saving…" : drafts.length ? "Save ingredient setup" : "Save empty setup"}</button></div>
      </form>
    </section>;
  }

  return <section className="review-shell costing-shell" aria-labelledby="ingredient-cost-heading">
    <button className="text-button" type="button" onClick={onBack}>← Back to menu</button>
    <p className="section-code">DB—MENU / CURRENT INGREDIENT COST</p>
    <h1 id="ingredient-cost-heading">{response.menuItem.name}</h1>
    <p>Current approximate ingredient cost per serving, using each ingredient's latest linked recorded purchase receipt when compatible.</p>
    {notice && <p className="success-notice" role="status">{notice}</p>}
    {error && <p className="form-error" role="alert">{error}</p>}
    <CostingSummaryCard summary={response.summary} sellingPrice={response.menuItem.sellingPrice}/>
    <div className="costing-result-actions"><button className="ledger-button" type="button" onClick={editSetup}>{response.ingredients.length ? "Edit ingredient setup" : "Set up ingredients"}</button>{response.ingredients.length > 0 && <button className="text-button" type="button" disabled={saving} onClick={stopTracking}>{saving ? "Stopping…" : "Stop tracking"}</button>}</div>
    {response.ingredients.length === 0 ? <div className="costing-empty"><h2>No ingredient setup yet</h2><p>Choose a few important ingredients to see a current approximate cost per serving.</p></div> : <div className="ingredient-cost-list">{response.ingredients.map(ingredient => <IngredientCostCard key={ingredient.id} ingredient={ingredient}/>)}</div>}
  </section>;
}

function CostingSummaryCard({ summary, sellingPrice }: { summary: CostingSummary; sellingPrice: string }) {
  if (summary.status === "complete") return <section className="costing-summary costing-summary-complete" aria-labelledby="costing-summary-heading"><p className="costing-status">Complete · all {summary.knownIngredientCount} ingredients have a compatible purchase</p><h2 id="costing-summary-heading">Approximate ingredient cost per serving</h2><strong>{exactCostMoney(summary.knownSubtotal, summary.currency)}</strong><p>{summary.approximateIngredientCostPercentage}% of the current selling price ({exactCostMoney(sellingPrice, summary.currency)}).</p></section>;
  if (summary.status === "partial") return <section className="costing-summary costing-summary-partial" aria-labelledby="costing-summary-heading"><p className="costing-status">Partial · {summary.knownIngredientCount} of {summary.configuredIngredientCount} ingredient costs available</p><h2 id="costing-summary-heading">Known ingredient subtotal per serving</h2><strong>{exactCostMoney(summary.knownSubtotal, summary.currency)}</strong><p>A selling-price percentage is not shown until every configured ingredient has a compatible recorded purchase.</p></section>;
  return <section className="costing-summary costing-summary-unavailable" aria-labelledby="costing-summary-heading"><p className="costing-status">Unavailable · {summary.knownIngredientCount} of {summary.configuredIngredientCount} ingredient costs available</p><h2 id="costing-summary-heading">No current ingredient cost available</h2><p>{summary.configuredIngredientCount === 0 ? "Set up ingredients to start this current estimate." : "Review each ingredient below, then use Connect purchases for missing or incompatible recorded receipts."}</p></section>;
}

function IngredientCostCard({ ingredient }: { ingredient: CostingIngredient }) {
  const calculation = ingredient.calculation;
  return <article className={`ingredient-cost-card ingredient-cost-${calculation.status}`}>
    <div className="ingredient-cost-heading"><div><p className="costing-status">{calculation.status === "available" ? "Available" : "Unavailable"}{ingredient.inventoryItemActive ? "" : " · Archived inventory item"}</p><h2>{ingredient.inventoryItemName}</h2><p>{ingredient.quantity} {servingUnitLabel(ingredient.unit)} per serving</p></div>{calculation.status === "available" && <strong>{exactCostMoney(calculation.costPerServing, calculation.currency)}</strong>}</div>
    {!ingredient.inventoryItemActive && <p className="review-warning" role="status">This configured inventory item is archived. It remains visible, but cannot be newly added.</p>}
    {calculation.status === "unavailable" ? <><p className="ingredient-recovery"><strong>Next move:</strong> {calculation.recovery}</p>{calculation.source && <CostSourceDetails source={calculation.source}/>}</> : <><p className="costing-formula"><span>Arithmetic</span>{calculation.arithmetic.formula}</p><CostSourceDetails source={calculation.source} arithmetic={calculation.arithmetic}/></>}
  </article>;
}

function CostSourceDetails({ source, arithmetic }: { source: CostSource; arithmetic?: CostArithmetic }) {
  return <details className="cost-source-details"><summary>Latest recorded purchase details</summary><div><p><strong>{source.supplierName}</strong> · {formatDate(source.invoiceDate)}</p><p>{source.description}</p><dl><div><dt>Purchase quantity</dt><dd>{source.purchaseQuantity ?? "Not recorded"} {source.purchaseUnit ?? ""}</dd></div><div><dt>Line total</dt><dd>{source.lineTotal === null ? "Not recorded" : exactCostMoney(source.lineTotal, source.currency)}</dd></div><div><dt>Unit price</dt><dd>{source.unitPrice === null ? "Not recorded" : exactCostMoney(source.unitPrice, source.currency)}</dd></div><div><dt>Purchase conversion</dt><dd>{source.countUnitsPerPurchaseUnit && source.countUnit ? `1 ${source.purchaseUnit ?? "purchase unit"} = ${source.countUnitsPerPurchaseUnit} ${source.countUnit}` : "Not usable"}</dd></div>{arithmetic && <><div><dt>Price used</dt><dd>{arithmetic.priceBasis === "lineTotalDividedByPurchaseQuantity" ? "Line total ÷ purchase quantity" : "Unit price"}</dd></div><div><dt>Cost per purchase unit</dt><dd>{exactCostMoney(arithmetic.purchaseUnitCost, source.currency)}</dd></div><div><dt>Serving amount in receipt unit</dt><dd>{arithmetic.ingredientQuantityInCountUnit} {source.countUnit}</dd></div></>}</dl></div></details>;
}

function costingDrafts(response: CostingResponse): IngredientDraft[] {
  return response.ingredients.map(ingredient => ({ inventoryItemId: ingredient.inventoryItemId, quantity: ingredient.quantity, unit: ingredient.unit, archived: !ingredient.inventoryItemActive, inventoryItemName: ingredient.inventoryItemName }));
}

function isPositiveIngredientQuantity(value: string) {
  const normalized = value.trim();
  const match = /^(\d+)(?:\.(\d+))?$/.exec(normalized);
  if (!match || (match[2]?.length ?? 0) > 6 || /^0+(?:\.0+)?$/.test(normalized)) return false;
  return (match[1].replace(/^0+/, "").length || 1) <= 12;
}

function exactCostMoney(value: string, currency: string) { return `${currency} ${value}`; }
function servingUnitLabel(unit: ServingUnit) { return servingUnits.find(option => option.value === unit)?.label ?? unit; }

function isValidMenuImportItem(item: MenuImportItem) {
  const price = item.sellingPrice?.trim() ?? "";
  return Boolean(item.name.trim()) && item.name.trim().length <= 50 && (item.category?.trim().length ?? 0) <= 20 && /^\d+(?:\.\d{1,4})?$/.test(price) && Number(price) > 0 && /^[A-Z]{3}$/.test(item.currency?.trim() ?? "");
}

function InvoiceWorkspace({ restaurant, request, active }: { restaurant: Restaurant; request: <T>(path: string, init?: RequestInit) => Promise<T>; active: boolean }) {
  const [invoices, setInvoices] = useState<Invoice[]>([]);
  const [loading, setLoading] = useState(true);
  const [listError, setListError] = useState("");
  const [supplier, setSupplier] = useState("");
  const [date, setDate] = useState(() => new Date().toLocaleDateString("en-CA"));
  const [file, setFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);
  const [notice, setNotice] = useState("");
  const [uploadError, setUploadError] = useState("");
  const [openingId, setOpeningId] = useState("");
  const [reviewId, setReviewId] = useState("");
  const [priceChangeId, setPriceChangeId] = useState("");
  const [purchaseId, setPurchaseId] = useState("");
  const [approvedPriceChanges, setApprovedPriceChanges] = useState<PriceChange[] | null>(null);
  const [retryingId, setRetryingId] = useState("");

  const loadInvoices = useCallback(() => {
    setLoading(true); setListError("");
    void request<Invoice[]>("/v1/invoices")
      .then(setInvoices)
      .catch((error: unknown) => setListError(error instanceof Error ? error.message : "Invoices couldn't load. Please try again."))
      .finally(() => setLoading(false));
  }, [request]);
  useEffect(() => { if (active) loadInvoices(); }, [active, loadInvoices]);
  useEffect(() => {
    if (!active || !invoices.some((invoice) => invoice.status === "processing")) return;
    const timer = window.setTimeout(loadInvoices, 15000);
    return () => window.clearTimeout(timer);
  }, [active, invoices, loadInvoices]);

  async function upload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setNotice(""); setUploadError("");
    const form = event.currentTarget;
    if (!supplier.trim() || !date || !file) { setUploadError("Add the supplier, invoice date, and a PDF or photo."); return; }
    if (file.size > 10 * 1024 * 1024) { setUploadError("Choose a file smaller than 10 MiB."); return; }
    const body = new FormData(); body.append("supplierName", supplier); body.append("invoiceDate", date); body.append("file", file);
    setUploading(true);
    try {
      const invoice = await request<Invoice>("/v1/invoices", { method: "POST", body });
      setInvoices((current) => [invoice, ...current]); setSupplier(""); setFile(null);
      setNotice("Invoice uploaded successfully.");
      form.reset(); setDate(new Date().toLocaleDateString("en-CA"));
    } catch (error) { setUploadError(error instanceof Error ? error.message : "Invoice upload failed. Please try again."); }
    finally { setUploading(false); }
  }

  async function openOriginal(invoice: Invoice) {
    setOpeningId(invoice.id); setListError("");
    const popup = window.open("", "_blank");
    if (popup) popup.opener = null;
    try {
      const { url } = await request<{ url: string }>(`/v1/invoices/${invoice.id}/file`);
      if (popup) popup.location.href = url; else window.open(url, "_blank", "noopener,noreferrer");
    } catch (error) { popup?.close(); setListError(error instanceof Error ? error.message : "The original couldn't open. Please try again."); }
    finally { setOpeningId(""); }
  }

  async function retry(invoice: Invoice) {
    setRetryingId(invoice.id); setListError("");
    try { await request(`/v1/invoices/${invoice.id}/retry`, { method: "POST" }); loadInvoices(); }
    catch (error) { setListError(error instanceof Error ? error.message : "The invoice couldn't be retried."); }
    finally { setRetryingId(""); }
  }

  if (reviewId) return <ReviewInvoice invoiceId={reviewId} request={request} onBack={() => { setReviewId(""); loadInvoices(); }} onApproved={(changes) => { const approvedId=reviewId; setReviewId(""); loadInvoices(); setNotice(changes.length?"":"Invoice approved. No price changes to flag."); if(changes.length){setApprovedPriceChanges(changes);setPriceChangeId(approvedId);}else{window.scrollTo({top:0});} }} onViewOriginal={() => { const invoice=invoices.find((item)=>item.id===reviewId); if(invoice) void openOriginal(invoice); }} />;
  if (priceChangeId) return <PriceChanges invoiceId={priceChangeId} request={request} initialChanges={approvedPriceChanges} onBack={() => { setPriceChangeId(""); setApprovedPriceChanges(null); loadInvoices(); }} />;
  if (purchaseId) return <ConnectPurchases invoiceId={purchaseId} request={request} onBack={() => { setPurchaseId(""); loadInvoices(); }} />;

  return <section className="invoice-workspace" aria-labelledby="invoices-heading">
    <header className="invoice-heading"><p className="section-code">DB—INVOICES</p><h1>{restaurant.name}</h1><p>{restaurant.city} · {formatServiceStyle(restaurant.serviceStyle)}</p></header>
    <div className="invoice-grid">
      <form className="invoice-form" onSubmit={upload} noValidate>
        <h2>Upload an invoice</h2><p>Save one supplier PDF or photo. Maximum 10 MiB.</p>
        <div className="ledger-field"><label htmlFor="supplier-name">Supplier name</label><input id="supplier-name" value={supplier} onChange={(event) => setSupplier(event.target.value)} maxLength={120} required /></div>
        <div className="ledger-field"><label htmlFor="invoice-date">Invoice date</label><input id="invoice-date" type="date" value={date} max={new Date().toLocaleDateString("en-CA")} onChange={(event) => setDate(event.target.value)} required /></div>
        <div className="file-actions">
          <input className="visually-hidden" id="invoice-camera" type="file" accept="image/jpeg,image/png,image/webp" capture="environment" onChange={(event) => setFile(event.target.files?.[0] ?? null)} /><label className="file-button" htmlFor="invoice-camera">Take a photo</label>
          <input className="visually-hidden" id="invoice-file" type="file" accept="application/pdf,image/jpeg,image/png,image/webp" onChange={(event) => setFile(event.target.files?.[0] ?? null)} /><label className="file-button" htmlFor="invoice-file">Choose a file</label>
        </div>
        {file && <p className="selected-file"><strong>Selected:</strong> {file.name} · {formatBytes(file.size)}</p>}
        {uploadError && <p className="form-error" role="alert">{uploadError}</p>}
        {notice && <p className="success-notice" role="status">{notice}</p>}
        <button className="ledger-button" type="submit" disabled={uploading}>{uploading ? "Uploading invoice…" : "Upload invoice"}<span aria-hidden="true">→</span></button>
      </form>
      <div className="invoice-list"><div className="list-heading"><h2 id="invoices-heading">Recent invoices</h2>{!loading && <button className="text-button" type="button" onClick={loadInvoices}>Refresh</button>}</div>
        {listError && <p className="form-error" role="alert">{listError}</p>}
        {loading ? <p role="status">Loading invoices…</p> : invoices.length === 0 ? <p className="empty-state">No invoices yet. Upload your first supplier invoice.</p> :
          <div className="invoice-cards">{invoices.map((invoice) => <article className="invoice-card" key={invoice.id}><div><p className="invoice-status">{importStatusLabel(invoice.status, invoice.delayed, "invoice")}</p><h3>{invoice.supplierName}</h3><p>{formatDate(invoice.invoiceDate)}</p><p className="invoice-filename">{invoice.originalFilename} · {formatBytes(invoice.sizeBytes)}</p></div><div className="card-actions">{invoice.status === "needs_review" && <button className="ledger-button" type="button" onClick={() => setReviewId(invoice.id)}>Review invoice</button>}{invoice.status === "failed" && <button className="ledger-button" type="button" disabled={retryingId===invoice.id} onClick={() => void retry(invoice)}>{retryingId===invoice.id ? "Trying again…" : "Try again"}</button>}{invoice.status === "ready" && invoice.priceChangeCount > 0 && <button className="price-change-alert" type="button" onClick={() => {setApprovedPriceChanges(null);setPriceChangeId(invoice.id)}}><span aria-hidden="true">!</span>{invoice.priceChangeCount} {invoice.priceChangeCount === 1 ? "price" : "prices"} changed</button>}{invoice.status === "ready" && <button className="ledger-button" type="button" onClick={() => setPurchaseId(invoice.id)}>Connect purchases</button>}<button className="file-button" type="button" disabled={openingId === invoice.id} onClick={() => void openOriginal(invoice)}>{openingId === invoice.id ? "Opening…" : "View original"}</button></div></article>)}</div>}
      </div>
    </div>
  </section>;
}

function importStatusLabel(status: string, delayed: boolean, document: "invoice" | "menu") {
  if (status === "processing") return delayed ? "Taking longer than usual" : `Reading ${document}…`;
  if (status === "failed") return "Couldn’t finish import";
  if (status === "needs_review") return "Ready to review";
  if (status === "ready") return "Ready";
  if (status === "imported") return "Imported";
  return status.replaceAll("_", " ");
}

function ReviewInvoice({ invoiceId, request, onBack, onApproved, onViewOriginal }: { invoiceId:string; request:<T>(path:string, init?:RequestInit)=>Promise<T>; onBack:()=>void; onApproved:(changes:PriceChange[])=>void; onViewOriginal:()=>void }) {
  const [review,setReview]=useState<Review|null>(null); const [error,setError]=useState(""); const [saving,setSaving]=useState(false);
  useEffect(()=>{ void request<Review>(`/v1/invoices/${invoiceId}/review`).then(setReview).catch((e:unknown)=>setError(e instanceof Error?e.message:"Review couldn't load.")); },[invoiceId,request]);
  function field(name:keyof Review,value:string){setReview((r)=>r?{...r,[name]:value||null}:r)}
  function line(index:number,name:keyof ReviewLine,value:string){setReview((r)=>r?{...r,lineItems:r.lineItems.map((item,i)=>i===index?{...item,[name]:value||null}:item)}:r)}
  async function submit(event:FormEvent){event.preventDefault();if(!review)return;setError("");if(!isValidInvoiceReview(review)){setError("Check the highlighted invoice fields and line items before approving.");return}setSaving(true);try{const {invoiceId:_,hasWarnings:__,...rest}=review;void _;void __;const payload={...rest,lineItems:rest.lineItems.map(({id,hasWarnings,...line})=>{void id;void hasWarnings;return line})};const result=await request<{priceChanges:PriceChange[]}>(`/v1/invoices/${invoiceId}/review`,{method:"PUT",body:JSON.stringify(payload)});onApproved(result.priceChanges);}catch(e){setError(e instanceof Error?e.message:"Review couldn't be saved. Check the values and try again.");setSaving(false)}}
  if(!review)return <section className="review-shell"><button className="text-button" onClick={onBack}>Back to invoices</button><p role={error?"alert":"status"}>{error||"Loading invoice…"}</p></section>;
  const textFields:[keyof Review,string][]=[["supplierName","Supplier"],["invoiceDate","Invoice date"],["invoiceNumber","Invoice number"],["currency","Currency"]];
  const totals:[keyof Review,string][]=[["subtotal","Subtotal"],["tax","Tax"],["fees","Fees"],["discount","Discount"],["total","Total"]];
  const headerNeedsAttention=review.hasWarnings||!isValidInvoiceHeader(review);
  return <section className="review-shell" aria-labelledby="review-heading"><div className="review-heading"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><button className="file-button" type="button" onClick={onViewOriginal}>View original</button></div><h1 id="review-heading">Review invoice</h1><p>Check every value against the original before approving.</p><form onSubmit={submit} className="review-form">{headerNeedsAttention&&<p className="review-warning" role="status">Some invoice details were unclear or invalid. Compare them with the original.</p>}<div className={`review-fields${headerNeedsAttention?" needs-attention":""}`}>{textFields.map(([name,label])=><label key={name}>{label}<input type={name==="invoiceDate"?"date":"text"} value={(review[name] as string|null)??""} onChange={(e)=>field(name,e.target.value)} maxLength={name==="supplierName"||name==="invoiceNumber"?120:name==="currency"?3:undefined} required={name==="supplierName"||name==="currency"}/></label>)}</div><fieldset><legend>Line items</legend>{review.lineItems.map((item,index)=>{const needsAttention=item.hasWarnings||!isValidInvoiceLine(item);return <div className={`line-card${needsAttention?" needs-attention":""}`} key={item.id??index}>{needsAttention&&<p className="review-warning" role="status">Check this line against the original invoice.</p>}<label>Description<input value={item.description} maxLength={500} onChange={(e)=>line(index,"description",e.target.value)} required/></label>{(["sku","quantity","unit","unitPrice","lineTotal"] as (keyof ReviewLine)[]).map((name)=><label key={name}>{name.replace(/([A-Z])/g," $1")}<input inputMode={name==="quantity"||name==="unitPrice"||name==="lineTotal"?"decimal":undefined} maxLength={name==="sku"?120:name==="unit"?40:32} value={(item[name] as string|null)??""} onChange={(e)=>line(index,name,e.target.value)}/></label>)}<button className="text-button" type="button" onClick={()=>setReview({...review,lineItems:review.lineItems.filter((_,i)=>i!==index)})}>Remove row</button></div>})}<button className="file-button" type="button" onClick={()=>setReview({...review,lineItems:[...review.lineItems,{sku:null,description:"",quantity:null,unit:null,unitPrice:null,lineTotal:null,hasWarnings:true}]})}>Add row</button></fieldset><div className="review-fields totals">{totals.map(([name,label])=><label key={name}>{label}<input inputMode="decimal" maxLength={32} value={(review[name] as string|null)??""} onChange={(e)=>field(name,e.target.value)}/></label>)}</div>{error&&<p className="form-error" role="alert">{error}</p>}<button className="ledger-button" disabled={saving}>{saving?"Approving…":"Approve invoice"}</button></form></section>;
}

function isValidInvoiceHeader(review:Review){return Boolean(review.supplierName.trim())&&review.supplierName.trim().length<=120&&/^[A-Z]{3}$/.test(review.currency.trim())&&(review.invoiceNumber?.trim().length??0)<=120&&[review.subtotal,review.tax,review.fees,review.discount,review.total].every(value=>isValidOptionalDecimal(value,4))}
function isValidInvoiceLine(line:ReviewLine){return Boolean(line.description.trim())&&line.description.trim().length<=500&&(line.sku?.trim().length??0)<=120&&(line.unit?.trim().length??0)<=40&&isValidOptionalDecimal(line.quantity,6)&&isValidOptionalDecimal(line.unitPrice,4)&&isValidOptionalDecimal(line.lineTotal,4)}
function isValidInvoiceReview(review:Review){return isValidInvoiceHeader(review)&&review.lineItems.length<=200&&review.lineItems.every(isValidInvoiceLine)}
function isValidOptionalDecimal(value:string|null,scale:number){if(!value?.trim())return true;const normalized=value.trim();const match=/^[+-]?(\d+)(?:\.(\d*))?$/.exec(normalized);return Boolean(match&&normalized.length<=32&&(match[2]?.length??0)<=scale&&match[1].replace(/^0+/,"").length<=18-scale)}

function PriceChanges({ invoiceId, request, initialChanges, onBack }: { invoiceId:string; request:<T>(path:string, init?:RequestInit)=>Promise<T>; initialChanges:PriceChange[]|null; onBack:()=>void }) {
  const [changes,setChanges]=useState<PriceChange[]|null>(initialChanges); const [error,setError]=useState("");
  useEffect(()=>{if(initialChanges)return;void request<PriceChange[]>(`/v1/invoices/${invoiceId}/price-changes`).then(setChanges).catch((reason:unknown)=>setError(reason instanceof Error?reason.message:"Price changes couldn't load."));},[initialChanges,invoiceId,request]);
  return <section className="review-shell price-change-shell" aria-labelledby="price-change-heading"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><p className="section-code">DB—COST CHECK</p><h1 id="price-change-heading">Price changes</h1>{initialChanges&&<p className="success-notice" role="status">Invoice approved. {initialChanges.length} {initialChanges.length===1?"price":"prices"} changed.</p>}<p>Compared with the last approved invoice from the same supplier.</p>{error?<p className="form-error" role="alert">{error}</p>:changes===null?<p role="status">Checking approved purchases…</p>:changes.length===0?<div className="price-empty"><h2>No price changes to flag.</h2><p>Prices are either within 5% of the last comparable purchase or there is not enough matching history yet. We only compare the same supplier, item, currency, and unit.</p></div>:<div className="price-change-list">{changes.map((change)=>{const percentage=Number(change.percentageChange);const increased=percentage>0;return <article className="price-change-card" key={change.id}><p className="invoice-status">{increased?"Cost increase":"Cost decrease"} · High confidence</p><h2>{change.description} is {increased?"up":"down"} {Math.abs(percentage).toFixed(1)}%</h2><p className="price-comparison"><strong>{formatMoney(change.currentUnitPrice,change.currency)}</strong><span>was {formatMoney(change.previousUnitPrice,change.currency)}{change.unit?` per ${change.unit}`:""}</span></p><p>Last comparable purchase: {formatDate(change.previousInvoiceDate)}.</p><p className="price-action"><strong>Next move:</strong> {increased?"Check the next delivery price or ask the supplier what changed.":"Use the lower cost when planning the next order."}</p></article>})}</div>}</section>;
}

function ConnectPurchases({ invoiceId, request, onBack }: { invoiceId:string; request:<T>(path:string, init?:RequestInit)=>Promise<T>; onBack:()=>void }) {
  const [response,setResponse]=useState<PurchaseResponse|null>(null); const [decisions,setDecisions]=useState<Record<string,PurchaseDecision>>({}); const [error,setError]=useState(""); const [saving,setSaving]=useState(false);
  useEffect(()=>{void request<PurchaseResponse>(`/v1/invoices/${invoiceId}/purchase-review`).then(value=>{setResponse(value);if(value.status==="pending"){setDecisions(Object.fromEntries(value.lines.map(line=>{const item=value.inventoryItems.find(candidate=>candidate.id===line.suggestedInventoryItemId);return [line.id,item&&line.suggestedConversion?{action:"match",inventoryItemId:item.id,expectedCountUnit:item.countUnit,conversion:line.suggestedConversion,suggested:true}:{action:""}]})));}}).catch((reason:unknown)=>setError(reason instanceof Error?reason.message:"Purchases couldn't load. Try again."));},[invoiceId,request]);
  function choose(line:PurchaseLine,action:"match"|"create"|"ignore"){if(action==="ignore"){setDecisions(value=>({...value,[line.id]:{action:"ignore"}}));return}if(action==="match"){const pending=response?.status==="pending"?response:null;const item=pending?.inventoryItems[0];setDecisions(value=>({...value,[line.id]:{action:"match",inventoryItemId:item?.id??"",expectedCountUnit:item?.countUnit??"",conversion:"1",suggested:false}}));return}setDecisions(value=>({...value,[line.id]:{action:"create",name:line.description.slice(0,50),category:"",countUnit:"each",conversion:"1"}}));}
  function update(lineId:string,patch:Record<string,string>){setDecisions(value=>({...value,[lineId]:{...value[lineId],...patch,suggested:false} as PurchaseDecision}));}
  async function submit(event:FormEvent){event.preventDefault();if(!response||response.status!=="pending")return;setError("");const invalid=response.lines.some(line=>!validPurchaseDecision(decisions[line.id],line));if(invalid){setError("Choose how to handle every line and check each conversion.");return}setSaving(true);try{const resolutions=response.lines.map(line=>{const decision=decisions[line.id];if(decision.action==="match"){return {lineId:line.id,action:decision.action,inventoryItemId:decision.inventoryItemId,expectedCountUnit:decision.expectedCountUnit,conversion:decision.conversion}}return {lineId:line.id,...decision}});const next=await request<PurchaseResponse>(`/v1/invoices/${invoiceId}/purchase-receipt`,{method:"PUT",body:JSON.stringify({resolutions})});setResponse(next);window.scrollTo({top:0});}catch(reason){setError(reason instanceof Error?reason.message:"The purchase receipt couldn't be recorded. Check the lines and try again.");}finally{setSaving(false)}}
  if(!response)return <section className="review-shell"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><p role={error?"alert":"status"}>{error||"Opening purchases…"}</p></section>;
  if(response.status==="recorded")return <ReceiptSummary receipt={response.receipt} replay={response.alreadyRecorded} onBack={onBack}/>;
  return <section className="review-shell purchase-shell" aria-labelledby="purchase-heading"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><p className="section-code">DB—PURCHASES</p><h1 id="purchase-heading">Connect purchases</h1><p>Choose what each invoice line should connect to. Nothing changes your inventory count.</p><div className="purchase-invoice-meta"><strong>{response.invoice.supplierName}</strong><span>{formatDate(response.invoice.invoiceDate)} · {response.invoice.invoiceNumber||"No invoice number"}</span></div><form className="purchase-form" onSubmit={submit}>{response.lines.map(line=>{const decision=decisions[line.id]??{action:""};const suggested=decision.action==="match"&&decision.suggested;const selectedItem=decision.action==="match"?response.inventoryItems.find(item=>item.id===decision.inventoryItemId):null;const countUnit=decision.action==="create"?decision.countUnit:selectedItem?.countUnit;return <fieldset className="purchase-line" key={line.id}><legend>{line.description}</legend><p className="purchase-source">{line.quantity??"No quantity"} {line.unit??"No purchase unit"}{line.lineTotal?` · ${formatMoney(line.lineTotal,response.invoice.currency)}`:""}</p>{suggested&&<p className="saved-match" role="status">Saved from past invoices · Review and record to confirm</p>}{!line.canTrack&&<p className="review-warning">This line needs a positive quantity and purchase unit to connect. You can choose not to track it.</p>}<div className="purchase-options"><label><input type="radio" name={`action-${line.id}`} checked={decision.action==="match"} disabled={!line.canTrack||response.inventoryItems.length===0} onChange={()=>choose(line,"match")}/> Match an inventory item</label><label><input type="radio" name={`action-${line.id}`} checked={decision.action==="create"} disabled={!line.canTrack} onChange={()=>choose(line,"create")}/> Create an inventory item</label><label><input type="radio" name={`action-${line.id}`} checked={decision.action==="ignore"} onChange={()=>choose(line,"ignore")}/> Don’t track this item</label></div>{decision.action==="match"&&<div className="purchase-fields"><label>Inventory item<select value={decision.inventoryItemId} onChange={event=>{const item=response.inventoryItems.find(candidate=>candidate.id===event.target.value);update(line.id,{inventoryItemId:event.target.value,expectedCountUnit:item?.countUnit??""})}}>{response.inventoryItems.map(item=><option key={item.id} value={item.id}>{item.name} · {item.countUnit}</option>)}</select></label><ConversionField line={line} countUnit={countUnit??"count units"} value={decision.conversion} onChange={value=>update(line.id,{conversion:value})}/></div>}{decision.action==="create"&&<div className="purchase-fields"><label>Item name<input maxLength={50} value={decision.name} onChange={event=>update(line.id,{name:event.target.value})}/></label><label>Category <span>Optional</span><input maxLength={20} value={decision.category} onChange={event=>update(line.id,{category:event.target.value})}/></label><label>Count unit<select value={decision.countUnit} onChange={event=>update(line.id,{countUnit:event.target.value})}>{inventoryUnits.map(unit=><option key={unit}>{unit}</option>)}</select></label><ConversionField line={line} countUnit={decision.countUnit} value={decision.conversion} onChange={value=>update(line.id,{conversion:value})}/></div>}</fieldset>})}{error&&<p className="form-error" role="alert">{error}</p>}<div className="purchase-submit"><button className="file-button" type="button" disabled={saving} onClick={onBack}>Save nothing and go back</button><button className="ledger-button" disabled={saving}>{saving?"Recording…":"Record purchase receipt"}</button></div></form></section>;
}

function ConversionField({line,countUnit,value,onChange}:{line:PurchaseLine;countUnit:string;value:string;onChange:(value:string)=>void}){const converted=convertedQuantity(line.quantity,value);return <label>Conversion<span className="conversion-input">1 {line.unit} = <input aria-label={`${line.description}, count units per ${line.unit}`} inputMode="decimal" value={value} onChange={event=>onChange(event.target.value)}/> {countUnit}</span>{converted&&<small>{line.quantity} {line.unit} will be recorded as {converted} {countUnit}</small>}</label>}
function validPurchaseDecision(decision:PurchaseDecision|undefined,line:PurchaseLine){if(!decision||!decision.action)return false;if(decision.action==="ignore")return true;if(!line.canTrack||!isPositiveDecimal(decision.conversion,12))return false;if(decision.action==="match")return Boolean(decision.inventoryItemId);return Boolean(decision.name.trim())&&decision.name.trim().length<=50&&decision.category.trim().length<=20&&Boolean(decision.countUnit.trim())}
function isPositiveDecimal(value:string,scale:number){const match=/^(\d+)(?:\.(\d*))?$/.exec(value.trim());return Boolean(match&&(match[2]?.length??0)<=scale&&Number(value)>0)}
function convertedQuantity(quantity:string|null,conversion:string){if(!quantity||!isPositiveDecimal(conversion,12))return "";const left=/^(\d+)(?:\.(\d*))?$/.exec(quantity.trim());const right=/^(\d+)(?:\.(\d*))?$/.exec(conversion.trim());if(!left||!right)return "";const leftFraction=left[2]??"";const rightFraction=right[2]??"";const product=(BigInt(left[1]+leftFraction)*BigInt(right[1]+rightFraction)).toString().padStart(leftFraction.length+rightFraction.length+1,"0");const scale=leftFraction.length+rightFraction.length;if(scale===0)return product;const exact=`${product.slice(0,-scale)}.${product.slice(-scale)}`.replace(/\.0+$/,"").replace(/(\.\d*?)0+$/,"$1");return exact.startsWith(".")?`0${exact}`:exact}
function ReceiptSummary({receipt,replay,onBack}:{receipt:PurchaseReceipt;replay:boolean;onBack:()=>void}){const tracked=receipt.lines.filter(line=>line.resolution!=="ignored").length;return <section className="review-shell receipt-shell" aria-labelledby="receipt-heading"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><p className="section-code">DB—PURCHASES / RECEIPT</p><h1 id="receipt-heading">Purchase receipt recorded</h1>{replay&&<p className="success-notice" role="status">This invoice was already recorded. Showing the saved receipt.</p>}<p>{receipt.invoice.supplierName} · {formatDate(receipt.invoice.invoiceDate)} · {tracked} connected, {receipt.lines.length-tracked} not tracked</p><div className="receipt-lines">{receipt.lines.map(line=><article key={line.id}><div><p className="invoice-status">{line.resolution==="ignored"?"Not tracked":line.resolution==="created"?"Inventory item created":"Matched to inventory"}</p><h2>{line.description}</h2><p>{line.quantity??"—"} {line.unit??""}{line.lineTotal?` · ${formatMoney(line.lineTotal,receipt.invoice.currency)}`:""}</p></div>{line.inventoryItemName&&<div className="receipt-link"><strong>{line.inventoryItemName}</strong><span>1 {line.unit} = {line.conversion} {line.countUnit}</span>{convertedQuantity(line.quantity,line.conversion??"")&&<span>Receipt quantity: {convertedQuantity(line.quantity,line.conversion??"")} {line.countUnit}</span>}</div>}</article>)}</div><button className="ledger-button" type="button" onClick={onBack}>Done</button></section>}

function formatBytes(bytes: number) { return bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`; }
function formatDate(value: string) { return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeZone: "UTC" }).format(new Date(`${value}T00:00:00Z`)); }
function formatMoney(value:string,currency:string){return new Intl.NumberFormat(undefined,{style:"currency",currency}).format(Number(value));}

function Onboarding({ onCreate, onSignOut }: { onCreate: (input: { name: string; city: string; serviceStyle: ServiceStyle }) => Promise<void>; onSignOut: () => void }) {
  const [name, setName] = useState("");
  const [city, setCity] = useState("");
  const [serviceStyle, setServiceStyle] = useState<ServiceStyle>("fast_casual");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!name.trim() || !city.trim()) { setError("Add your restaurant name and city to continue."); return; }
    setSubmitting(true); setError("");
    try { await onCreate({ name, city, serviceStyle }); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "We couldn't open your Daybook. Please try again."); setSubmitting(false); }
  }

  return <main className="app-shell">
    <AppHeader onSignOut={onSignOut} />
    <section className="onboarding-shell" aria-labelledby="onboarding-heading">
      <p className="section-code">DB—SETUP / 01</p>
      <h1 id="onboarding-heading">Open your <em>restaurant daybook.</em></h1>
      <p className="brief-intro">Three details now. Invoices, ingredients, and the rest can wait until your next shift.</p>
      <form className="onboarding-form" onSubmit={submit} noValidate>
        <div className="ledger-field"><label htmlFor="restaurant-name">Restaurant name</label><p id="name-help">Use the name your crew knows.</p><input id="restaurant-name" value={name} onChange={(event) => setName(event.target.value)} maxLength={120} autoComplete="organization" aria-describedby="name-help form-error" required /></div>
        <div className="ledger-field"><label htmlFor="city">City</label><p id="city-help">The city for this first location.</p><input id="city" value={city} onChange={(event) => setCity(event.target.value)} maxLength={100} autoComplete="address-level2" aria-describedby="city-help form-error" required /></div>
        <div className="ledger-field"><label htmlFor="service-style">Service style</label><p id="style-help">Choose the closest fit. You can keep setup simple.</p><select id="service-style" value={serviceStyle} onChange={(event) => setServiceStyle(event.target.value as ServiceStyle)} aria-describedby="style-help form-error">{serviceStyles.map((style) => <option key={style.value} value={style.value}>{style.label}</option>)}</select></div>
        {error && <p className="form-error" id="form-error" role="alert">{error}</p>}
        <button className="ledger-button" type="submit" disabled={submitting}>{submitting ? "Opening Daybook…" : "Open my Daybook"}<span aria-hidden="true">→</span></button>
      </form>
    </section>
  </main>;
}

function ErrorPage({ message, onRetry, onSignOut }: { message: string; onRetry: () => void; onSignOut: () => void }) {
  return <main className="status-page"><div className="error-notice" role="alert"><p className="section-code">DB—CONNECTION</p><h1>We couldn't open the brief.</h1><p>{message}</p><button className="ledger-button ledger-button-light" type="button" onClick={onRetry}>Retry <span aria-hidden="true">→</span></button><button className="text-button" type="button" onClick={onSignOut}>Sign out</button></div></main>;
}

function formatServiceStyle(value: ServiceStyle) { return serviceStyles.find((style) => style.value === value)?.label ?? value; }

type WelcomeProps = {
  authConfigured: boolean;
  onSignIn?: () => Promise<void>;
  onSignUp?: () => Promise<void>;
};

function Welcome({ authConfigured, onSignIn, onSignUp }: WelcomeProps) {
  return (
    <main className="landing-shell">
      <header className="landing-header">
        <Wordmark />
        <p className="header-note">The daily operating brief<br />for independent restaurants</p>
        <div className="header-actions">
          {authConfigured ? (
            <button className="text-button" type="button" onClick={onSignIn}>Operator sign in</button>
          ) : (
            <span className="edition-label">Dallas pilot · 01</span>
          )}
        </div>
      </header>

      <section className="landing-hero" aria-labelledby="hero-heading">
        <div className="hero-copy">
          <p className="hero-kicker"><span>Service intelligence</span><span>Not another dashboard</span></p>
          <h1 id="hero-heading"><span className="hero-command">Run the <span>shift.</span></span><em>Protect the margin.</em></h1>
          <p className="hero-lede">Daybook keeps invoice reviews, supplier price evidence, and inventory count follow-ups in one short list.</p>

          <div className="hero-actions">
            <button className="ledger-button ledger-button-light" type="button" onClick={onSignUp} disabled={!authConfigured}>
              Start your daybook <span aria-hidden="true">→</span>
            </button>
            <span>Keep your POS.<br />Skip the spreadsheet.</span>
          </div>

          <div className="signal-chain" aria-label="How Daybook works">
            <span>01 · Snap invoices</span>
            <span>02 · Count what matters</span>
            <span>03 · Work the brief</span>
          </div>
        </div>

        <ServiceBrief />
      </section>

      <footer className="landing-footer">
        <span>Invoices → cost changes → action</span>
        <strong>Built for the back office that is actually a corner of the kitchen.</strong>
        <span>Dallas pilot · Edition 01</span>
      </footer>
    </main>
  );
}

function Wordmark() {
  return (
    <a className="wordmark" href="/" aria-label="Daybook home">
      <span className="wordmark-index">DB<br />01</span>
      <span className="wordmark-name">Daybook</span>
    </a>
  );
}

function AppHeader({ onSignOut, restaurantName }: { onSignOut: () => void; restaurantName?: string }) {
  return (
    <header className="app-header">
      <Wordmark />
      <p className="restaurant-label"><span>Restaurant</span>{restaurantName ?? "New daybook"}</p>
      <button className="text-button" type="button" onClick={onSignOut}>Sign out</button>
    </header>
  );
}

function ServiceBrief() {
  return (
    <aside className="service-brief" aria-label="Sample Friday service brief">
      <div className="brief-pin" aria-hidden="true" />
      <header className="service-brief-header">
        <div>
          <p>Sample service brief</p>
          <h2>Friday<br />Dinner</h2>
        </div>
        <div className="date-block date-block-dark" aria-label="Friday, July 17">
          <strong>17</strong>
          <span>JUL<br />FRI</span>
        </div>
      </header>

      <p className="brief-summary"><strong>3 source-backed actions.</strong> Review the records, then choose what to do.</p>

      <ol className="brief-list">
        <li>
          <span className="task-number">01</span>
          <div><strong>Review 2 supplier invoices</strong><small>Both records still need owner review</small></div>
          <span className="task-mark task-mark-urgent">Urgent</span>
        </li>
        <li>
          <span className="task-number">02</span>
          <div><strong>Check the chicken invoice price</strong><small>Up 11% from the last comparable invoice</small></div>
          <span className="task-mark">Invoice</span>
        </li>
        <li>
          <span className="task-number">03</span>
          <div><strong>Resume inventory count</strong><small>A saved draft is ready to continue</small></div>
          <span className="task-mark">Count</span>
        </li>
      </ol>

      <footer className="service-brief-footer">
        <span>Generated 7:04 AM</span>
        <span className="confidence-stamp">Source<br />backed</span>
        <span>Daybook / DB-0717</span>
      </footer>
    </aside>
  );
}

function StatusPage({ message }: { message: string }) {
  return <main className="status-page" role="status">{message}</main>;
}
