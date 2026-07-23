import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";

type Role = "owner" | "manager" | "staff";
type ServiceStyle = "counter_service" | "full_service" | "fast_casual" | "cafe_bakery" | "bar";

export type SettingsRestaurant = {
  id: string;
  name: string;
  city: string;
  serviceStyle: ServiceStyle;
  timezone: string;
  role: Role;
};

type TeamMember = {
  id: string;
  email: string | null;
  displayName: string | null;
  role: Role;
  isCurrentUser: boolean;
};

type TeamInvitation = {
  id: string;
  email: string;
  role: "manager" | "staff";
  expiresAt: string;
};

type SettingsResponse = {
  restaurant: SettingsRestaurant;
  team: TeamMember[] | null;
  invitations: TeamInvitation[] | null;
  invitationsEnabled: boolean;
};

type Draft = Pick<SettingsRestaurant, "name" | "city" | "serviceStyle" | "timezone">;

type Props = {
  restaurant: SettingsRestaurant;
  request: <T>(path: string, init?: RequestInit) => Promise<T>;
  active: boolean;
  onRestaurantChange: (restaurant: SettingsRestaurant) => void;
};

const serviceStyles: { value: ServiceStyle; label: string }[] = [
  { value: "counter_service", label: "Counter service" },
  { value: "full_service", label: "Full service" },
  { value: "fast_casual", label: "Fast casual" },
  { value: "cafe_bakery", label: "Cafe/Bakery" },
  { value: "bar", label: "Bar" },
];

const roles: { value: Role; label: string }[] = [
  { value: "owner", label: "Owner" },
  { value: "manager", label: "Manager" },
  { value: "staff", label: "Staff" },
];

const invitationRoles: { value: TeamInvitation["role"]; label: string }[] = [
  { value: "manager", label: "Manager" },
  { value: "staff", label: "Staff" },
];

const commonTimezones = [
  "America/New_York",
  "America/Chicago",
  "America/Denver",
  "America/Phoenix",
  "America/Los_Angeles",
  "America/Anchorage",
  "Pacific/Honolulu",
  "America/Toronto",
  "America/Vancouver",
  "Europe/London",
  "Europe/Paris",
  "Asia/Dubai",
  "Asia/Singapore",
  "Australia/Sydney",
  "UTC",
];

export function SettingsWorkspace({ restaurant, request, active, onRestaurantChange }: Props) {
  const [response, setResponse] = useState<SettingsResponse | null>(null);
  const [draft, setDraft] = useState<Draft>(() => restaurantDraft(restaurant));
  const [roleDrafts, setRoleDrafts] = useState<Record<string, Role>>({});
  const [inviteOpen, setInviteOpen] = useState(false);
  const [inviteEmail, setInviteEmail] = useState("");
  const [inviteRole, setInviteRole] = useState<TeamInvitation["role"]>("staff");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [sendingInvite, setSendingInvite] = useState(false);
  const [busyMemberId, setBusyMemberId] = useState("");
  const [busyInvitationId, setBusyInvitationId] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  const adopt = useCallback((value: SettingsResponse) => {
    setResponse(value);
    setDraft(restaurantDraft(value.restaurant));
    setRoleDrafts(Object.fromEntries((value.team ?? []).map(member => [member.id, member.role])));
    onRestaurantChange(value.restaurant);
  }, [onRestaurantChange]);

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      adopt(await request<SettingsResponse>("/v1/settings"));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Settings couldn't load. Try again.");
    } finally {
      setLoading(false);
    }
  }, [adopt, request]);

  useEffect(() => { if (active) void load(); }, [active, load]);

  async function saveRestaurant(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError("");
    setNotice("");
    if (!draft.name.trim() || !draft.city.trim()) {
      setError("Add the restaurant name and city.");
      return;
    }
    if (!isSupportedTimezone(draft.timezone.trim())) {
      setError("Enter a valid IANA timezone, such as America/Chicago.");
      return;
    }
    setSaving(true);
    try {
      adopt(await request<SettingsResponse>("/v1/settings", {
        method: "PUT",
        body: JSON.stringify(draft),
      }));
      setNotice("Settings saved. New local-day and weekly boundaries will use this timezone.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Settings couldn't be saved. Try again.");
    } finally {
      setSaving(false);
    }
  }

  async function saveRole(member: TeamMember) {
    const nextRole = roleDrafts[member.id] ?? member.role;
    if (nextRole === member.role) return;
    if (member.role === "owner" && nextRole !== "owner" && !window.confirm(`Change ${memberLabel(member)} from owner to ${roleLabel(nextRole)}? At least one owner must remain.`)) return;
    setBusyMemberId(member.id);
    setError("");
    setNotice("");
    try {
      adopt(await request<SettingsResponse>(`/v1/settings/team/${member.id}/role`, {
        method: "PUT",
        body: JSON.stringify({ role: nextRole }),
      }));
      setNotice(`${memberLabel(member)} now has ${roleLabel(nextRole)} access.`);
    } catch (cause) {
      setRoleDrafts(current => ({ ...current, [member.id]: member.role }));
      setError(cause instanceof Error ? cause.message : "That role couldn't be changed. Try again.");
    } finally {
      setBusyMemberId("");
    }
  }

  async function removeMember(member: TeamMember) {
    if (!window.confirm(`Remove ${memberLabel(member)} from this restaurant? They will immediately lose Parline access.`)) return;
    setBusyMemberId(member.id);
    setError("");
    setNotice("");
    try {
      adopt(await request<SettingsResponse>(`/v1/settings/team/${member.id}`, { method: "DELETE" }));
      setNotice(`${memberLabel(member)} no longer has access to this restaurant.`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "That team member couldn't be removed. Try again.");
    } finally {
      setBusyMemberId("");
    }
  }

  async function sendInvitation(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const email = inviteEmail.trim();
    if (!email) {
      setError("Enter the teammate's email address.");
      return;
    }
    setSendingInvite(true);
    setError("");
    setNotice("");
    try {
      adopt(await request<SettingsResponse>("/v1/settings/invitations", {
        method: "POST",
        body: JSON.stringify({ email, role: inviteRole }),
      }));
      setInviteEmail("");
      setInviteRole("staff");
      setInviteOpen(false);
      setNotice(`Invitation sent to ${email}. They must accept the email invitation before access is added.`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The invitation couldn't be sent. Check the email and try again.");
    } finally {
      setSendingInvite(false);
    }
  }

  async function resendInvitation(invitation: TeamInvitation) {
    setBusyInvitationId(invitation.id);
    setError("");
    setNotice("");
    try {
      adopt(await request<SettingsResponse>(`/v1/settings/invitations/${invitation.id}/resend`, { method: "POST" }));
      setNotice(`A new invitation email was sent to ${invitation.email}.`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The invitation couldn't be resent. Try again.");
    } finally {
      setBusyInvitationId("");
    }
  }

  async function revokeInvitation(invitation: TeamInvitation) {
    if (!window.confirm(`Revoke the invitation for ${invitation.email}? Their invitation link will stop working.`)) return;
    setBusyInvitationId(invitation.id);
    setError("");
    setNotice("");
    try {
      adopt(await request<SettingsResponse>(`/v1/settings/invitations/${invitation.id}`, { method: "DELETE" }));
      setNotice(`Invitation for ${invitation.email} revoked.`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The invitation couldn't be revoked. Try again.");
    } finally {
      setBusyInvitationId("");
    }
  }

  const ownerCount = useMemo(() => response?.team?.filter(member => member.role === "owner").length ?? 0, [response]);
  const timezoneOptions = useMemo(
    () => availableTimezones(response?.restaurant.timezone ?? restaurant.timezone),
    [response?.restaurant.timezone, restaurant.timezone],
  );

  if (loading) return <section className="settings-workspace"><p className="settings-status" role="status">Loading settings…</p></section>;
  if (!response) return <section className="settings-workspace"><div className="settings-load-error"><p className="form-error" role="alert">{error || "Settings couldn't load."}</p><button className="file-button" type="button" onClick={() => void load()}>Try again</button></div></section>;

  const owner = response.restaurant.role === "owner";
  return <section className="settings-workspace" aria-labelledby="settings-heading">
    <header className="settings-heading">
      <h1 id="settings-heading">Settings</h1>
      <p>{owner ? "Keep the details that shape each operating day accurate." : "Review the restaurant details used across Parline."}</p>
    </header>

    {error && <p className="form-error settings-message" role="alert">{error}</p>}
    {notice && <p className="success-notice settings-message" role="status">{notice}</p>}

    {owner ? <form className="settings-form" onSubmit={saveRestaurant} noValidate>
      <div className="settings-fields">
        <label>Restaurant name<input required autoComplete="organization" maxLength={50} value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })}/></label>
        <label>City<input required autoComplete="address-level2" maxLength={100} value={draft.city} onChange={event => setDraft({ ...draft, city: event.target.value })}/></label>
        <label>Service style<select value={draft.serviceStyle} onChange={event => setDraft({ ...draft, serviceStyle: event.target.value as ServiceStyle })}>{serviceStyles.map(style => <option key={style.value} value={style.value}>{style.label}</option>)}</select></label>
        <label>Timezone<select required value={draft.timezone} aria-describedby="timezone-help" onChange={event => setDraft({ ...draft, timezone: event.target.value })}>{timezoneOptions.map(timezone => <option key={timezone} value={timezone}>{timezone.replaceAll("_", " ")}</option>)}</select><small id="timezone-help">Choose the restaurant's local timezone. Future Today and weekly brief boundaries change; saved timestamps and records do not.</small></label>
      </div>
      <button className="ledger-button" disabled={saving}>{saving ? "Saving settings…" : "Save settings"}</button>
    </form> : <div className="settings-readonly">
      <p className="readonly-note">Only an owner can update these details or manage team access.</p>
      <dl>
        <div><dt>Restaurant name</dt><dd>{response.restaurant.name}</dd></div>
        <div><dt>City</dt><dd>{response.restaurant.city}</dd></div>
        <div><dt>Service style</dt><dd>{serviceStyleLabel(response.restaurant.serviceStyle)}</dd></div>
        <div><dt>Timezone</dt><dd>{response.restaurant.timezone}</dd></div>
      </dl>
      <p className="timezone-note">This timezone sets future Today and weekly brief boundaries. Existing timestamps and saved records stay unchanged.</p>
    </div>}

    {owner && <section className="team-settings" aria-labelledby="team-heading">
      <div className="team-heading">
        <div><p className="section-code">Access</p><h2 id="team-heading">Team access</h2></div>
        <div className="team-heading-actions">
          <span>{response.team?.length ?? 0} {(response.team?.length ?? 0) === 1 ? "member" : "members"}</span>
          <button className="ledger-button" type="button" disabled={!response.invitationsEnabled || inviteOpen} onClick={() => { setInviteOpen(true); setError(""); setNotice(""); }}>Invite teammate</button>
        </div>
      </div>
      <p>Choose access around the work each person does. Only owners can change roles or remove access.</p>
      <div className="role-guide" aria-label="Role access summary">
        <p><strong>Owner</strong><span>Everything, including supplier price details, the weekly brief, restaurant settings, and team access.</span></p>
        <p><strong>Manager</strong><span>Invoices, menus, sales imports, inventory setup and counts, ingredient costing, waste, and stockouts.</span></p>
        <p><strong>Staff</strong><span>Daily sales entry, inventory counts, waste, and stockouts—without financial reports or setup controls.</span></p>
      </div>
      {!response.invitationsEnabled && <p className="invite-note">Team invitations have not been enabled for this Parline environment yet. Existing team access can still be managed below.</p>}

      {inviteOpen && <form className="invite-form" onSubmit={sendInvitation} noValidate>
        <div className="invite-form-heading"><div><p className="section-code">New invitation</p><h3>Invite a teammate</h3></div><button className="text-button" type="button" disabled={sendingInvite} onClick={() => { setInviteOpen(false); setInviteEmail(""); }}>Cancel</button></div>
        <p>They will receive a secure email link. Access begins only after they accept it and sign in with the invited email.</p>
        <div className="invite-fields">
          <label htmlFor="invite-email">Email address<input id="invite-email" type="email" inputMode="email" autoComplete="email" required maxLength={254} placeholder="teammate@example.com" value={inviteEmail} onChange={event => setInviteEmail(event.target.value)}/></label>
          <label htmlFor="invite-role">Access<select id="invite-role" value={inviteRole} aria-describedby="invite-role-help" onChange={event => setInviteRole(event.target.value as TeamInvitation["role"])}>{invitationRoles.map(role => <option key={role.value} value={role.value}>{role.label}</option>)}</select><small id="invite-role-help">{inviteRole === "manager" ? "Runs invoices, menu, sales imports, inventory, costing, and shift logs." : "Records daily sales, counts, waste, and stockouts."}</small></label>
        </div>
        <button className="ledger-button" disabled={sendingInvite}>{sendingInvite ? "Sending invitation…" : "Send invitation"}</button>
      </form>}

      {(response.invitations?.length ?? 0) > 0 && <section className="pending-invitations" aria-labelledby="pending-invitations-heading">
        <div className="list-heading"><div><p className="section-code">Waiting for acceptance</p><h3 id="pending-invitations-heading">Pending invitations</h3></div><span>{response.invitations?.length}</span></div>
        <div className="pending-invitation-list">{response.invitations?.map(invitation => <article className="pending-invitation-card" key={invitation.id}>
          <div><p className="invoice-status">{roleLabel(invitation.role)} access</p><h4>{invitation.email}</h4><p>Expires {formatInvitationExpiry(invitation.expiresAt)}</p></div>
          <div className="pending-invitation-actions"><button className="file-button" type="button" disabled={Boolean(busyInvitationId)} onClick={() => void resendInvitation(invitation)}>{busyInvitationId === invitation.id ? "Working…" : "Resend email"}</button><button className="text-button" type="button" disabled={Boolean(busyInvitationId)} onClick={() => void revokeInvitation(invitation)}>Revoke</button></div>
        </article>)}</div>
      </section>}

      <div className="team-list">{response.team?.map(member => {
        const draftRole = roleDrafts[member.id] ?? member.role;
        const changingLastOwner = member.role === "owner" && ownerCount <= 1;
        return <article className="team-card" key={member.id}>
          <div className="team-identity"><p className="invoice-status">{member.isCurrentUser ? "You" : roleLabel(member.role)}</p><h3>{memberDisplayName(member)}</h3><p>{member.email ?? "Email unavailable"}</p></div>
          <div className="team-actions">
            <label htmlFor={`role-${member.id}`}>Role<select id={`role-${member.id}`} value={draftRole} disabled={busyMemberId === member.id} onChange={event => setRoleDrafts(current => ({ ...current, [member.id]: event.target.value as Role }))}>{roles.map(role => <option key={role.value} value={role.value} disabled={changingLastOwner && role.value !== "owner"}>{role.label}</option>)}</select></label>
            <button className="file-button" type="button" disabled={busyMemberId === member.id || draftRole === member.role} onClick={() => void saveRole(member)}>{busyMemberId === member.id ? "Saving…" : "Save role"}</button>
            {!member.isCurrentUser && <button className="text-button" type="button" disabled={Boolean(busyMemberId)} onClick={() => void removeMember(member)}>Remove access</button>}
          </div>
          {changingLastOwner && <p className="owner-safety-note">This is the last owner. Promote another member before changing this role.</p>}
        </article>;
      })}</div>
    </section>}
  </section>;
}

function restaurantDraft(restaurant: SettingsRestaurant): Draft {
  return { name: restaurant.name, city: restaurant.city, serviceStyle: restaurant.serviceStyle, timezone: restaurant.timezone };
}

function isSupportedTimezone(value: string) {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return Boolean(value);
  } catch {
    return false;
  }
}

function availableTimezones(current: string) {
  const supported = typeof Intl.supportedValuesOf === "function"
    ? Intl.supportedValuesOf("timeZone")
    : commonTimezones;
  return [...new Set([...supported, ...commonTimezones, current].filter(Boolean))]
    .sort((left, right) => left.localeCompare(right));
}

function memberDisplayName(member: TeamMember) {
  return member.displayName?.trim() || (member.isCurrentUser ? "Your account" : member.email?.trim()) || "Team member";
}

function memberLabel(member: TeamMember) {
  return member.isCurrentUser ? "your account" : member.displayName?.trim() || member.email?.trim() || "this team member";
}

function formatInvitationExpiry(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

function roleLabel(role: Role) { return roles.find(value => value.value === role)?.label ?? role; }
function serviceStyleLabel(style: ServiceStyle) { return serviceStyles.find(value => value.value === style)?.label ?? style; }
