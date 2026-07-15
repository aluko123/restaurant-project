import { useEffect, useState } from "react";
import { useAuth } from "@workos-inc/authkit-react";

type AppProps = {
  authConfigured: boolean;
};

type ApiStatus = "checking" | "ready" | "unavailable";

export function App({ authConfigured }: AppProps) {
  return authConfigured ? <AuthenticatedApp /> : <Welcome authConfigured={false} />;
}

function AuthenticatedApp() {
  const { isLoading, user, signIn, signUp, signOut } = useAuth();

  if (isLoading) {
    return <StatusPage message="Checking your session…" />;
  }

  if (!user) {
    return <Welcome authConfigured onSignIn={signIn} onSignUp={signUp} />;
  }

  return (
    <main className="shell">
      <header className="topbar">
        <a className="brand" href="/" aria-label="Restaurant Copilot home">
          <span className="brand-mark" aria-hidden="true">R</span>
          <span>Restaurant Copilot</span>
        </a>
        <button className="button button-quiet" type="button" onClick={() => signOut()}>
          Sign out
        </button>
      </header>

      <section className="today" aria-labelledby="today-heading">
        <p className="eyebrow">Today</p>
        <h1 id="today-heading">Welcome, {user.firstName ?? user.email}</h1>
        <p className="lede">Your restaurant setup is the next step. We’ll start with the details needed to make useful daily recommendations.</p>
        <div className="action-card">
          <div>
            <p className="action-label">First action</p>
            <h2>Set up your restaurant</h2>
            <p>Add the location, service style, and a few important ingredients. You can skip the full inventory for now.</p>
          </div>
          <button className="button button-primary" type="button" disabled>
            Coming next
          </button>
        </div>
      </section>
      <ApiHealth />
    </main>
  );
}

type WelcomeProps = {
  authConfigured: boolean;
  onSignIn?: () => Promise<void>;
  onSignUp?: () => Promise<void>;
};

function Welcome({ authConfigured, onSignIn, onSignUp }: WelcomeProps) {
  return (
    <main className="welcome-shell">
      <nav className="topbar" aria-label="Primary navigation">
        <a className="brand" href="/" aria-label="Restaurant Copilot home">
          <span className="brand-mark" aria-hidden="true">R</span>
          <span>Restaurant Copilot</span>
        </a>
        {authConfigured && (
          <button className="button button-quiet" type="button" onClick={onSignIn}>
            Sign in
          </button>
        )}
      </nav>

      <section className="hero" aria-labelledby="hero-heading">
        <div>
          <p className="eyebrow">Daily profit copilot</p>
          <h1 id="hero-heading">Know what to buy, what to prep, and where profit is leaking.</h1>
          <p className="lede">Keep your POS. Snap supplier invoices, count the ingredients that matter, and get a short list of actions for today.</p>
          {authConfigured ? (
            <div className="button-row">
              <button className="button button-primary" type="button" onClick={onSignUp}>
                Set up your restaurant
              </button>
              <button className="button button-secondary" type="button" onClick={onSignIn}>
                Sign in
              </button>
            </div>
          ) : (
            <div className="setup-note" role="status">
              <strong>Authentication is not configured.</strong>
              <span>Add a WorkOS client ID to `apps/web/.env` to enable Google and email sign-in.</span>
            </div>
          )}
        </div>

        <aside className="preview-card" aria-label="Example daily actions">
          <p className="preview-date">Friday, July 17</p>
          <h2>3 actions for today</h2>
          <ol className="action-list">
            <li><span className="number">1</span><span><strong>Order 6 cases of tortillas</strong><small>Likely to run out before Saturday dinner</small></span></li>
            <li><span className="number">2</span><span><strong>Review chicken taco margin</strong><small>Chicken cost rose 11% this week</small></span></li>
            <li><span className="number">3</span><span><strong>Count avocados</strong><small>Last count was four days ago</small></span></li>
          </ol>
        </aside>
      </section>
    </main>
  );
}

function ApiHealth() {
  const [status, setStatus] = useState<ApiStatus>("checking");
  const apiUrl = import.meta.env.VITE_API_URL ?? "http://localhost:8080";

  useEffect(() => {
    const controller = new AbortController();
    fetch(`${apiUrl}/health/ready`, { signal: controller.signal })
      .then((response) => setStatus(response.ok ? "ready" : "unavailable"))
      .catch((error: unknown) => {
        if (!(error instanceof DOMException && error.name === "AbortError")) {
          setStatus("unavailable");
        }
      });
    return () => controller.abort();
  }, [apiUrl]);

  return <p className={`api-status api-status-${status}`}>API: {status}</p>;
}

function StatusPage({ message }: { message: string }) {
  return <main className="status-page" role="status">{message}</main>;
}
