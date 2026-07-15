import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AuthKitProvider } from "@workos-inc/authkit-react";
import { App } from "./App";
import "./styles.css";

const root = createRoot(document.getElementById("root")!);
const workosClientId = import.meta.env.VITE_WORKOS_CLIENT_ID?.trim();

root.render(
  <StrictMode>
    {workosClientId ? (
      <AuthKitProvider clientId={workosClientId}>
        <App authConfigured />
      </AuthKitProvider>
    ) : (
      <App authConfigured={false} />
    )}
  </StrictMode>,
);
