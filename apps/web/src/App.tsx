import { FormEvent, useCallback, useEffect, useState } from "react";
import { useAuth } from "@workos-inc/authkit-react";

type AppProps = {
  authConfigured: boolean;
};

type Restaurant = { id: string; name: string; city: string; serviceStyle: ServiceStyle; role: string };
type Invoice = { id: string; supplierName: string; invoiceDate: string; originalFilename: string; contentType: string; sizeBytes: number; status: string; delayed: boolean; createdAt: string };
type ReviewLine = { id?: string; sku: string | null; description: string; quantity: string | null; unit: string | null; unitPrice: string | null; lineTotal: string | null; hasWarnings: boolean };
type Review = { invoiceId: string; supplierName: string; invoiceNumber: string | null; invoiceDate: string | null; currency: string; subtotal: string | null; tax: string | null; fees: string | null; discount: string | null; total: string | null; hasWarnings: boolean; lineItems: ReviewLine[] };
type PriceChange = { id: string; description: string; unit: string | null; currency: string; previousUnitPrice: string; currentUnitPrice: string; percentageChange: string; previousInvoiceDate: string };
type MenuItem = { id: string; name: string; category: string | null; sellingPrice: string; currency: string; active: boolean };
type MenuImport = { id: string; originalFilename: string; status: string; delayed: boolean; createdAt: string };
type MenuImportItem = { id?: string; name: string; category: string | null; sellingPrice: string | null; currency: string | null; hasWarnings: boolean; selected?: boolean };
type InventoryItem = { id: string; name: string; category: string | null; countUnit: string; parLevel: string | null; active: boolean; latestQuantity: string | null; previousQuantity: string | null; change: string | null; lastCountedAt: string | null; lowStock: boolean };
type InventoryCountEntry = { id: string; inventoryItemId: string; name: string; category: string | null; countUnit: string; quantity: string | null };
type InventoryCount = { id: string; status: string; revision: number; createdAt: string; updatedAt: string; completedAt: string | null; entries: InventoryCountEntry[] };
type InventoryDraftResponse = { count: InventoryCount | null };
type ServiceStyle = "counter_service" | "full_service" | "fast_casual" | "cafe_bakery" | "bar";
type AppState = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; restaurant: Restaurant | null };

const serviceStyles: { value: ServiceStyle; label: string }[] = [
  { value: "counter_service", label: "Counter service" },
  { value: "full_service", label: "Full service" },
  { value: "fast_casual", label: "Fast casual" },
  { value: "cafe_bakery", label: "Cafe/Bakery" },
  { value: "bar", label: "Bar" },
];

export function App({ authConfigured }: AppProps) {
  return authConfigured ? <AuthenticatedApp /> : <Welcome authConfigured={false} />;
}

function AuthenticatedApp() {
  const { isLoading, user, signIn, signUp, signOut, getAccessToken } = useAuth();
  const [appState, setAppState] = useState<AppState>({ status: "loading" });
  type Workspace = "invoices" | "menu" | "inventory";
  const workspaceForPath = (): Workspace => window.location.pathname === "/menu" ? "menu" : window.location.pathname === "/inventory" ? "inventory" : "invoices";
  const [workspace, setWorkspace] = useState<Workspace>(workspaceForPath);
  const apiUrl = import.meta.env.VITE_API_URL ?? "http://localhost:8080";

  useEffect(() => {
    if (!isLoading && !user && window.location.pathname === "/login") {
      const context = new URLSearchParams(window.location.search).get("context") ?? undefined;
      void signIn({ context });
    }
  }, [isLoading, signIn, user]);

  const request = useCallback(async <T,>(path: string, init?: RequestInit): Promise<T> => {
    const token = await getAccessToken();
    const headers = new Headers(init?.headers);
    if (!(init?.body instanceof FormData)) headers.set("Content-Type", "application/json");
    headers.set("Authorization", `Bearer ${token}`);
    const response = await fetch(`${apiUrl}${path}`, {
      ...init,
      headers,
    });
    const body = await response.json().catch(() => null) as { error?: string } | null;
    if (!response.ok) throw new Error(body?.error ?? "Daybook couldn't reach the kitchen. Please try again.");
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

  function openWorkspace(next: Workspace) {
    const path = `/${next}`;
    if (window.location.pathname !== path) window.history.pushState({}, "", path);
    setWorkspace(next);
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
        <button type="button" aria-current={workspace === "invoices" ? "page" : undefined} onClick={() => openWorkspace("invoices")}>Invoices</button>
        <button type="button" aria-current={workspace === "menu" ? "page" : undefined} onClick={() => openWorkspace("menu")}>Menu</button>
        <button type="button" aria-current={workspace === "inventory" ? "page" : undefined} onClick={() => openWorkspace("inventory")}>Inventory</button>
      </nav>
      <div hidden={workspace !== "invoices"}><InvoiceWorkspace restaurant={restaurant} request={request} active={workspace === "invoices"} /></div>
      <div hidden={workspace !== "menu"}><MenuWorkspace restaurant={restaurant} request={request} active={workspace === "menu"} /></div>
      <div hidden={workspace !== "inventory"}><InventoryWorkspace restaurant={restaurant} request={request} /></div>
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

function MenuWorkspace({ restaurant, request, active }: { restaurant: Restaurant; request: <T>(path: string, init?: RequestInit) => Promise<T>; active: boolean }) {
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
  useEffect(() => { if (active) loadImports(); }, [active, loadImports]);
  useEffect(() => {
    if (!active || !imports.some((menuImport) => menuImport.status === "processing")) return;
    const timer = window.setTimeout(loadImports, 15000);
    return () => window.clearTimeout(timer);
  }, [active, imports, loadImports]);
  async function submit(event:FormEvent<HTMLFormElement>){event.preventDefault();setError("");setNotice("");if(!name.trim()||!price.trim()){setError("Add the menu item name and selling price.");return}setSaving(true);try{const item=await request<MenuItem>("/v1/menu-items",{method:"POST",body:JSON.stringify({name,category:category||null,sellingPrice:price,currency})});setItems((current)=>[...current,item].sort((a,b)=>(a.category??"zzz").localeCompare(b.category??"zzz")||a.name.localeCompare(b.name)));setName("");setCategory("");setPrice("");setNotice(`${item.name} added to the menu.`);}catch(reason){setError(reason instanceof Error?reason.message:"The menu item couldn't be saved.");}finally{setSaving(false)}}
  async function uploadMenu(e:FormEvent<HTMLFormElement>){e.preventDefault();setError("");setNotice("");if(!importFile){setError("Choose one menu photo or PDF.");return}if(importFile.size>10*1024*1024){setError("Choose a file smaller than 10 MiB.");return}const body=new FormData();body.append("file",importFile);setUploading(true);try{const value=await request<MenuImport>("/v1/menu-imports",{method:"POST",body});setImports(v=>[value,...v]);setImportFile(null);setNotice("Menu uploaded. Extraction is processing.");(e.currentTarget as HTMLFormElement).reset()}catch(reason){setError(reason instanceof Error?reason.message:"Menu upload failed. Try again.")}finally{setUploading(false)}}
  async function openReview(id:string){setError("");try{const value=await request<{import:MenuImport;items:MenuImportItem[]}>(`/v1/menu-imports/${id}`);setReview({...value,items:value.items.map(v=>({...v,selected:isValidMenuImportItem(v)}))})}catch(reason){setError(reason instanceof Error?reason.message:"Review couldn't open.")}}
  async function retryMenu(menuImport:MenuImport){setRetryingId(menuImport.id);setError("");try{await request(`/v1/menu-imports/${menuImport.id}/retry`,{method:"POST"});loadImports()}catch(reason){setError(reason instanceof Error?reason.message:"The menu couldn't be retried.")}finally{setRetryingId("")}}
  async function openOriginal(menuImport:MenuImport){setOpeningId(menuImport.id);setError("");const popup=window.open("","_blank");if(popup)popup.opener=null;try{const {url}=await request<{url:string}>(`/v1/menu-imports/${menuImport.id}/file`);if(popup)popup.location.href=url;else window.open(url,"_blank","noopener,noreferrer")}catch(reason){popup?.close();setError(reason instanceof Error?reason.message:"The original couldn't open. Please try again.")}finally{setOpeningId("")}}
  async function approve(){if(!review)return;setError("");const selected=review.items.filter(v=>v.selected);if(!selected.length){setError("Select at least one valid item.");return}if(selected.some(v=>!isValidMenuImportItem(v))){setError("Check each selected item's name, positive price, and three-letter currency.");return}setSaving(true);try{const counts=await request<{imported:number;skipped:number}>(`/v1/menu-imports/${review.import.id}`,{method:"PUT",body:JSON.stringify({items:selected.map(({id,name,category,sellingPrice,currency})=>({id,name,category,sellingPrice,currency}))})});setNotice(`${counts.imported} items imported${counts.skipped?`; ${counts.skipped} duplicates skipped`:""}.`);setReview(null);loadMenu();loadImports()}catch(reason){setError(reason instanceof Error?reason.message:"Items couldn't be imported.")}finally{setSaving(false)}}
  if(review)return <section className="review-shell"><button className="text-button" type="button" onClick={()=>{setError("");setReview(null)}}>← Back to menu</button><h1>Review menu</h1><p>Edit and select items with a clear selling price. Nothing is added until you import.</p><div className="review-form">{review.items.map((row,index)=>{const needsAttention=row.hasWarnings||!isValidMenuImportItem(row);return <article className={`line-card${needsAttention?" needs-attention":""}`} key={row.id??`new-${index}`}>{needsAttention&&<p className="review-warning" role="status">Check this item against the original menu.</p>}<label><input type="checkbox" checked={Boolean(row.selected)} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,selected:e.target.checked}:x)}))}/> Import this item</label><label>Name<input value={row.name} maxLength={50} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,name:e.target.value}:x)}))}/></label><label>Category<input value={row.category??""} maxLength={20} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,category:e.target.value||null}:x)}))}/></label><label>Price<input inputMode="decimal" value={row.sellingPrice??""} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,sellingPrice:e.target.value||null}:x)}))}/></label><label>Currency<input maxLength={3} value={row.currency??""} onChange={e=>setReview(v=>v&&({...v,items:v.items.map((x,i)=>i===index?{...x,currency:e.target.value.toUpperCase()||null}:x)}))}/></label><button className="text-button" type="button" onClick={()=>setReview(v=>v&&({...v,items:v.items.filter((_,i)=>i!==index)}))}>Remove row</button></article>})}<button className="file-button" type="button" onClick={()=>setReview(v=>v&&({...v,items:[...v.items,{name:"",category:null,sellingPrice:null,currency:"USD",hasWarnings:true,selected:true}]}))}>Add item</button>{error&&<p className="form-error" role="alert">{error}</p>}<button className="ledger-button" type="button" disabled={saving} onClick={approve}>{saving?"Importing…":"Import selected items"}</button></div></section>;
  return <section className="menu-workspace" aria-labelledby="menu-heading"><header className="invoice-heading"><p className="section-code">DB—MENU / TOP ITEMS</p><h1 id="menu-heading">{restaurant.name} menu</h1><p>Add items by hand or review one menu photo or PDF.</p></header><div className="menu-grid"><div><form className="invoice-form menu-form" onSubmit={submit} noValidate><h2>Add a menu item</h2><p>Record the name customers see and its current selling price.</p><div className="ledger-field"><label htmlFor="menu-name">Menu item</label><input id="menu-name" value={name} onChange={e=>setName(e.target.value)} maxLength={50} required/></div><div className="ledger-field"><label htmlFor="menu-category">Category <span className="optional-label">Optional</span></label><input id="menu-category" value={category} onChange={e=>setCategory(e.target.value)} maxLength={20}/></div><div className="menu-price-fields"><div className="ledger-field"><label htmlFor="menu-price">Selling price</label><input id="menu-price" inputMode="decimal" value={price} onChange={e=>setPrice(e.target.value)} required/></div><div className="ledger-field"><label htmlFor="menu-currency">Currency</label><select id="menu-currency" value={currency} onChange={e=>setCurrency(e.target.value)}><option>USD</option><option>CAD</option><option>GBP</option><option>EUR</option></select></div></div><button className="ledger-button" disabled={saving}>{saving?"Adding item…":"Add to menu"}</button></form><form className="invoice-form menu-import-form" onSubmit={uploadMenu}><h2>Import from photo</h2><p>Upload one PDF or photo. You'll review every item before it is added.</p><label className="file-button" htmlFor="menu-file">Choose photo or PDF</label><input className="visually-hidden" id="menu-file" type="file" accept="application/pdf,image/jpeg,image/png,image/webp" onChange={e=>setImportFile(e.target.files?.[0]??null)}/>{importFile&&<span className="selected-file">{importFile.name}</span>}<button className="ledger-button" disabled={uploading}>{uploading?"Uploading…":"Upload menu"}</button></form>{error&&<p className="form-error" role="alert">{error}</p>}{notice&&<p className="success-notice" role="status">{notice}</p>}</div><div className="menu-list"><div className="list-heading"><h2>Active menu items</h2><button className="text-button" type="button" onClick={loadMenu}>Refresh</button></div>{loading?<p role="status">Loading menu…</p>:items.length===0?<p className="empty-state">No menu items yet.</p>:<div className="menu-cards">{items.map(item=><article className="menu-card" key={item.id}><div><p className="invoice-status">{item.category??"Uncategorized"}</p><h3>{item.name}</h3></div><strong>{formatMoney(item.sellingPrice,item.currency)}</strong></article>)}</div>}<div className="list-heading import-heading"><h2>Menu imports</h2></div>{imports.length===0?<p className="empty-state">No menu imports yet.</p>:<div className="menu-cards">{imports.map(value=><article className="menu-card" key={value.id}><div className="menu-card-copy"><p className="invoice-status">{importStatusLabel(value.status, value.delayed, "menu")}</p><h3>{value.originalFilename}</h3></div><div className="card-actions">{value.status==="needs_review"&&<button className="file-button" type="button" onClick={()=>void openReview(value.id)}>Review menu</button>}{value.status==="failed"&&<button className="file-button" type="button" disabled={retryingId===value.id} onClick={()=>void retryMenu(value)}>{retryingId===value.id?"Trying again…":"Retry"}</button>}<button className="text-button" type="button" disabled={openingId===value.id} onClick={()=>void openOriginal(value)}>{openingId===value.id?"Opening…":"Original"}</button></div></article>)}</div>}</div></div></section>;
}

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

  if (reviewId) return <ReviewInvoice invoiceId={reviewId} request={request} onBack={() => { setReviewId(""); loadInvoices(); }} onApproved={() => { setReviewId(""); setPriceChangeId(reviewId); loadInvoices(); }} onViewOriginal={() => { const invoice=invoices.find((item)=>item.id===reviewId); if(invoice) void openOriginal(invoice); }} />;
  if (priceChangeId) return <PriceChanges invoiceId={priceChangeId} request={request} onBack={() => { setPriceChangeId(""); loadInvoices(); }} />;

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
          <div className="invoice-cards">{invoices.map((invoice) => <article className="invoice-card" key={invoice.id}><div><p className="invoice-status">{importStatusLabel(invoice.status, invoice.delayed, "invoice")}</p><h3>{invoice.supplierName}</h3><p>{formatDate(invoice.invoiceDate)}</p><p className="invoice-filename">{invoice.originalFilename} · {formatBytes(invoice.sizeBytes)}</p></div><div className="card-actions">{invoice.status === "needs_review" && <button className="ledger-button" type="button" onClick={() => setReviewId(invoice.id)}>Review invoice</button>}{invoice.status === "failed" && <button className="ledger-button" type="button" disabled={retryingId===invoice.id} onClick={() => void retry(invoice)}>{retryingId===invoice.id ? "Trying again…" : "Try again"}</button>}{invoice.status === "ready" && <button className="ledger-button" type="button" onClick={() => setPriceChangeId(invoice.id)}>Price changes</button>}<button className="file-button" type="button" disabled={openingId === invoice.id} onClick={() => void openOriginal(invoice)}>{openingId === invoice.id ? "Opening…" : "View original"}</button></div></article>)}</div>}
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

function ReviewInvoice({ invoiceId, request, onBack, onApproved, onViewOriginal }: { invoiceId:string; request:<T>(path:string, init?:RequestInit)=>Promise<T>; onBack:()=>void; onApproved:()=>void; onViewOriginal:()=>void }) {
  const [review,setReview]=useState<Review|null>(null); const [error,setError]=useState(""); const [saving,setSaving]=useState(false);
  useEffect(()=>{ void request<Review>(`/v1/invoices/${invoiceId}/review`).then(setReview).catch((e:unknown)=>setError(e instanceof Error?e.message:"Review couldn't load.")); },[invoiceId,request]);
  function field(name:keyof Review,value:string){setReview((r)=>r?{...r,[name]:value||null}:r)}
  function line(index:number,name:keyof ReviewLine,value:string){setReview((r)=>r?{...r,lineItems:r.lineItems.map((item,i)=>i===index?{...item,[name]:value||null}:item)}:r)}
  async function submit(event:FormEvent){event.preventDefault();if(!review)return;setError("");if(!isValidInvoiceReview(review)){setError("Check the highlighted invoice fields and line items before approving.");return}setSaving(true);try{const {invoiceId:_,hasWarnings:__,...rest}=review;void _;void __;const payload={...rest,lineItems:rest.lineItems.map(({id,hasWarnings,...line})=>{void id;void hasWarnings;return line})};await request(`/v1/invoices/${invoiceId}/review`,{method:"PUT",body:JSON.stringify(payload)});onApproved();}catch(e){setError(e instanceof Error?e.message:"Review couldn't be saved. Check the values and try again.");setSaving(false)}}
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

function PriceChanges({ invoiceId, request, onBack }: { invoiceId:string; request:<T>(path:string, init?:RequestInit)=>Promise<T>; onBack:()=>void }) {
  const [changes,setChanges]=useState<PriceChange[]|null>(null); const [error,setError]=useState("");
  useEffect(()=>{void request<PriceChange[]>(`/v1/invoices/${invoiceId}/price-changes`).then(setChanges).catch((reason:unknown)=>setError(reason instanceof Error?reason.message:"Price changes couldn't load."));},[invoiceId,request]);
  return <section className="review-shell price-change-shell" aria-labelledby="price-change-heading"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><p className="section-code">DB—COST CHECK</p><h1 id="price-change-heading">Price changes</h1><p>Compared with the last approved invoice from the same supplier.</p>{error?<p className="form-error" role="alert">{error}</p>:changes===null?<p role="status">Checking approved purchases…</p>:changes.length===0?<div className="price-empty"><h2>No price changes to flag.</h2><p>Prices are either within 5% of the last comparable purchase or there is not enough matching history yet. We only compare the same supplier, item, currency, and unit.</p></div>:<div className="price-change-list">{changes.map((change)=>{const percentage=Number(change.percentageChange);const increased=percentage>0;return <article className="price-change-card" key={change.id}><p className="invoice-status">{increased?"Cost increase":"Cost decrease"} · High confidence</p><h2>{change.description} is {increased?"up":"down"} {Math.abs(percentage).toFixed(1)}%</h2><p className="price-comparison"><strong>{formatMoney(change.currentUnitPrice,change.currency)}</strong><span>was {formatMoney(change.previousUnitPrice,change.currency)}{change.unit?` per ${change.unit}`:""}</span></p><p>Last comparable purchase: {formatDate(change.previousInvoiceDate)}.</p><p className="price-action"><strong>Next move:</strong> {increased?"Check the next delivery price or ask the supplier what changed.":"Use the lower cost when planning the next order."}</p></article>})}</div>}</section>;
}

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
          <p className="hero-lede">Daybook turns supplier invoices, quick counts, and yesterday’s sales into the few decisions that matter before service.</p>

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

      <p className="brief-summary"><strong>3 moves before 4 PM.</strong> One protects service. Two protect margin.</p>

      <ol className="brief-list">
        <li>
          <span className="task-number">01</span>
          <div><strong>Order 6 cases of tortillas</strong><small>86% stockout risk · before 11 AM</small></div>
          <span className="task-mark task-mark-urgent">Order</span>
        </li>
        <li>
          <span className="task-number">02</span>
          <div><strong>Check the chicken taco</strong><small>Chicken is up 11% since last invoice</small></div>
          <span className="task-mark">Margin</span>
        </li>
        <li>
          <span className="task-number">03</span>
          <div><strong>Count avocados</strong><small>Last count Tuesday · 2 minute check</small></div>
          <span className="task-mark">Count</span>
        </li>
      </ol>

      <footer className="service-brief-footer">
        <span>Generated 7:04 AM</span>
        <span className="confidence-stamp">Medium<br />confidence</span>
        <span>Daybook / DB-0717</span>
      </footer>
    </aside>
  );
}

function StatusPage({ message }: { message: string }) {
  return <main className="status-page" role="status">{message}</main>;
}
