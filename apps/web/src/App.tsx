import { FormEvent, useCallback, useEffect, useState } from "react";
import { useAuth } from "@workos-inc/authkit-react";

type AppProps = {
  authConfigured: boolean;
};

type Restaurant = { id: string; name: string; city: string; serviceStyle: ServiceStyle; role: string };
type ServiceStyle = "counter_service" | "full_service" | "fast_casual" | "cafe_bakery" | "bar";
type AppState = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; restaurant: Restaurant | null };

const serviceStyles: { value: ServiceStyle; label: string }[] = [
  { value: "counter_service", label: "Counter service" },
  { value: "full_service", label: "Full service" },
  { value: "fast_casual", label: "Fast casual" },
  { value: "cafe_bakery", label: "Cafe / bakery" },
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
    const response = await fetch(`${apiUrl}${path}`, {
      ...init,
      headers: { "Content-Type": "application/json", ...init?.headers, Authorization: `Bearer ${token}` },
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
      <section className="app-workspace" aria-labelledby="today-heading">
        <aside className="shift-rail" aria-label="Shift details">
          <span className="shift-rail-label">Opening brief</span>
          <span className="shift-rail-number">01</span>
          <span className="shift-rail-rule" aria-hidden="true" />
          <span>{restaurant.city}<br />{formatServiceStyle(restaurant.serviceStyle)}</span>
        </aside>

        <div className="today-brief">
          <header className="brief-heading">
            <div>
              <p className="section-code">DB—SETUP / TODAY</p>
              <h1 id="today-heading">Good morning, <em>{user.firstName ?? user.email}</em></h1>
            </div>
            <div className="date-block" aria-label="Friday, July 17">
              <strong>17</strong>
              <span>JUL<br />FRI</span>
            </div>
          </header>

          <p className="brief-intro"><strong>{restaurant.name}</strong> is ready. Your first brief will grow from real supplier and shift data—not placeholder analytics.</p>

          <div className="setup-action">
            <span className="setup-action-number">01</span>
            <div className="setup-action-copy">
              <p className="task-overline">First move · about 2 minutes</p>
              <h2>Upload your first invoice</h2>
              <p>No invoices yet. Upload your first supplier invoice to start tracking price changes for {restaurant.city}.</p>
            </div>
            <button className="ledger-button" type="button" disabled>
              Upload coming next <span aria-hidden="true">→</span>
            </button>
          </div>

          <div className="coming-up" aria-label="What comes after setup">
            <p>Then, your first service brief</p>
            <ul>
              <li>Supplier price movement</li>
              <li>Stockout risk</li>
              <li>Prep and order priorities</li>
            </ul>
          </div>
          <p className="restaurant-meta">{formatServiceStyle(restaurant.serviceStyle)} · {restaurant.city} · {restaurant.role}</p>
        </div>
      </section>
    </main>
  );
}

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
