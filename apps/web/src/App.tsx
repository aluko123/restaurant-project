import { FormEvent, useCallback, useEffect, useState } from "react";
import { useAuth } from "@workos-inc/authkit-react";

type AppProps = {
  authConfigured: boolean;
};

type Restaurant = { id: string; name: string; city: string; serviceStyle: ServiceStyle; role: string };
type Invoice = { id: string; supplierName: string; invoiceDate: string; originalFilename: string; contentType: string; sizeBytes: number; status: string; createdAt: string };
type ReviewLine = { id?: string; sku: string | null; description: string; quantity: string | null; unit: string | null; unitPrice: string | null; lineTotal: string | null };
type Review = { invoiceId: string; supplierName: string; invoiceNumber: string | null; invoiceDate: string | null; currency: string; subtotal: string | null; tax: string | null; fees: string | null; discount: string | null; total: string | null; lineItems: ReviewLine[] };
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
      <InvoiceWorkspace restaurant={restaurant} request={request} />
    </main>
  );
}

function InvoiceWorkspace({ restaurant, request }: { restaurant: Restaurant; request: <T>(path: string, init?: RequestInit) => Promise<T> }) {
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
  const [retryingId, setRetryingId] = useState("");

  const loadInvoices = useCallback(() => {
    setLoading(true); setListError("");
    void request<Invoice[]>("/v1/invoices")
      .then(setInvoices)
      .catch((error: unknown) => setListError(error instanceof Error ? error.message : "Invoices couldn't load. Please try again."))
      .finally(() => setLoading(false));
  }, [request]);
  useEffect(loadInvoices, [loadInvoices]);
  useEffect(() => {
    if (!invoices.some((invoice) => invoice.status === "processing")) return;
    const timer = window.setInterval(loadInvoices, 5000);
    return () => window.clearInterval(timer);
  }, [invoices, loadInvoices]);

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

  if (reviewId) return <ReviewInvoice invoiceId={reviewId} request={request} onBack={() => { setReviewId(""); loadInvoices(); }} onViewOriginal={() => { const invoice=invoices.find((item)=>item.id===reviewId); if(invoice) void openOriginal(invoice); }} />;

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
          <div className="invoice-cards">{invoices.map((invoice) => <article className="invoice-card" key={invoice.id}><div><p className="invoice-status">{statusText(invoice.status)}</p><h3>{invoice.supplierName}</h3><p>{formatDate(invoice.invoiceDate)}</p><p className="invoice-filename">{invoice.originalFilename} · {formatBytes(invoice.sizeBytes)}</p></div><div className="card-actions">{invoice.status === "needs_review" && <button className="ledger-button" type="button" onClick={() => setReviewId(invoice.id)}>Review invoice</button>}{invoice.status === "failed" && <button className="ledger-button" type="button" disabled={retryingId===invoice.id} onClick={() => void retry(invoice)}>{retryingId===invoice.id ? "Trying again…" : "Try again"}</button>}{invoice.status === "ready" && <span className="reviewed-label">Reviewed</span>}<button className="file-button" type="button" disabled={openingId === invoice.id} onClick={() => void openOriginal(invoice)}>{openingId === invoice.id ? "Opening…" : "View original"}</button></div></article>)}</div>}
      </div>
    </div>
  </section>;
}

function statusText(status: string) { return status === "needs_review" ? "Review invoice" : status === "ready" ? "Ready" : status === "failed" ? "Couldn’t read invoice" : "Processing"; }

function ReviewInvoice({ invoiceId, request, onBack, onViewOriginal }: { invoiceId:string; request:<T>(path:string, init?:RequestInit)=>Promise<T>; onBack:()=>void; onViewOriginal:()=>void }) {
  const [review,setReview]=useState<Review|null>(null); const [error,setError]=useState(""); const [saving,setSaving]=useState(false);
  useEffect(()=>{ void request<Review>(`/v1/invoices/${invoiceId}/review`).then(setReview).catch((e:unknown)=>setError(e instanceof Error?e.message:"Review couldn't load.")); },[invoiceId,request]);
  function field(name:keyof Review,value:string){setReview((r)=>r?{...r,[name]:value||null}:r)}
  function line(index:number,name:keyof ReviewLine,value:string){setReview((r)=>r?{...r,lineItems:r.lineItems.map((item,i)=>i===index?{...item,[name]:value||null}:item)}:r)}
  async function submit(event:FormEvent){event.preventDefault();if(!review)return;setSaving(true);setError("");try{const {invoiceId:_,...rest}=review;void _;const payload={...rest,lineItems:rest.lineItems.map(({id,...line})=>{void id;return line})};await request(`/v1/invoices/${invoiceId}/review`,{method:"PUT",body:JSON.stringify(payload)});onBack();}catch(e){setError(e instanceof Error?e.message:"Review couldn't be saved. Check the values and try again.");setSaving(false)}}
  if(!review)return <section className="review-shell"><button className="text-button" onClick={onBack}>Back to invoices</button><p role={error?"alert":"status"}>{error||"Loading invoice…"}</p></section>;
  const textFields:[keyof Review,string][]=[["supplierName","Supplier"],["invoiceDate","Invoice date"],["invoiceNumber","Invoice number"],["currency","Currency"]];
  const totals:[keyof Review,string][]=[["subtotal","Subtotal"],["tax","Tax"],["fees","Fees"],["discount","Discount"],["total","Total"]];
  return <section className="review-shell" aria-labelledby="review-heading"><div className="review-heading"><button className="text-button" type="button" onClick={onBack}>Back to invoices</button><button className="file-button" type="button" onClick={onViewOriginal}>View original</button></div><h1 id="review-heading">Review invoice</h1><p>Check every value against the original before approving.</p><form onSubmit={submit} className="review-form"> <div className="review-fields">{textFields.map(([name,label])=><label key={name}>{label}<input type={name==="invoiceDate"?"date":"text"} value={(review[name] as string|null)??""} onChange={(e)=>field(name,e.target.value)} required={name==="supplierName"||name==="currency"}/></label>)}</div><fieldset><legend>Line items</legend>{review.lineItems.map((item,index)=><div className="line-card" key={item.id??index}><label>Description<input value={item.description} onChange={(e)=>line(index,"description",e.target.value)} required/></label>{(["sku","quantity","unit","unitPrice","lineTotal"] as (keyof ReviewLine)[]).map((name)=><label key={name}>{name.replace(/([A-Z])/g," $1")}<input inputMode={name==="quantity"||name==="unitPrice"||name==="lineTotal"?"decimal":undefined} value={(item[name] as string|null)??""} onChange={(e)=>line(index,name,e.target.value)}/></label>)}<button className="text-button" type="button" onClick={()=>setReview({...review,lineItems:review.lineItems.filter((_,i)=>i!==index)})}>Remove row</button></div>)}<button className="file-button" type="button" onClick={()=>setReview({...review,lineItems:[...review.lineItems,{sku:null,description:"",quantity:null,unit:null,unitPrice:null,lineTotal:null}]})}>Add row</button></fieldset><div className="review-fields totals">{totals.map(([name,label])=><label key={name}>{label}<input inputMode="decimal" value={(review[name] as string|null)??""} onChange={(e)=>field(name,e.target.value)}/></label>)}</div>{error&&<p className="form-error" role="alert">{error}</p>}<button className="ledger-button" disabled={saving}>{saving?"Approving…":"Approve invoice"}</button></form></section>;
}

function formatBytes(bytes: number) { return bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`; }
function formatDate(value: string) { return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeZone: "UTC" }).format(new Date(`${value}T00:00:00Z`)); }

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
