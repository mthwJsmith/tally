import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  ArrowRight,
  RotateCw,
  Pencil,
  Trash2,
  CheckCircle2,
  AlertTriangle,
  Wallet,
} from "lucide-react";
import { api } from "@/lib/api";
import { relativeTime } from "@/lib/format";
import type { Account, Consent } from "@/types/api";

export const Route = createFileRoute("/banks")({ component: BanksPage });

function BanksPage() {
  const qc = useQueryClient();
  // Detect post-OAuth landing — if the URL has ?linked=<nickname>, the consent was just
  // created and a background sync is in flight. Poll until it lands.
  const urlParams = typeof window !== "undefined" ? new URLSearchParams(window.location.search) : new URLSearchParams();
  const justLinked = urlParams.get("linked");

  const consents = useQuery({
    queryKey: ["consents"],
    queryFn: () => api.get<{ consents: Consent[] }>("/api/sync/status"),
    refetchInterval: (q) => {
      const list = q.state.data?.consents ?? [];
      // Refetch every 3s while any consent has no sync_status yet, OR for ~45s after a
      // fresh link, OR if any sync just finished but hasn't propagated.
      const anyPending = list.some(
        (c) => !c.last_sync_status || c.last_sync_status === null
      );
      if (justLinked || anyPending) return 3000;
      return false;
    },
  });
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts"),
    refetchInterval: justLinked ? 3000 : false,
  });

  const [nickname, setNickname] = useState("");

  const rename = useMutation({
    mutationFn: ({ id, nickname }: { id: number; nickname: string }) =>
      api.patch(`/api/consents/${id}/rename`, { nickname }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["consents"] }),
  });

  const renameAccount = useMutation({
    mutationFn: ({ id, name }: { id: number; name: string | null }) =>
      api.patch(`/api/accounts/${id}`, { custom_display_name: name }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["accounts"] }),
  });

  const removeConsent = useMutation({
    mutationFn: (id: number) => api.delete(`/consents/${id}`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["consents"] });
      qc.invalidateQueries({ queryKey: ["accounts"] });
    },
  });

  const syncConsent = useMutation({
    mutationFn: (id: number) => api.post(`/consents/${id}/sync`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["consents"] }),
  });

  // Group accounts by consent for the cards.
  const accountsByConsent = (accounts.data?.accounts ?? []).reduce<
    Record<number, Account[]>
  >((acc, a) => {
    (acc[a.consent_id] = acc[a.consent_id] ?? []).push(a);
    return acc;
  }, {});

  return (
    <div className="p-8 md:p-12 space-y-8">
      <header className="fade-in">
        <p className="text-xs uppercase tracking-widest text-mid mb-3">Banks</p>
        <h1 className="text-4xl md:text-5xl mb-2">
          Linked <em>banks</em>
        </h1>
        <p className="text-mid text-sm max-w-2xl">
          Each bank links via TrueLayer with one OAuth flow. Single callback URL —
          <span className="mono text-xs ml-1">/auth/callback</span>. The nickname
          is just a label and can be renamed any time without re-linking.
        </p>
      </header>

      {justLinked && (
        <div className="card p-4 bg-green/5 border-green/30 flex items-center gap-3 fade-in">
          <RotateCw className="size-4 text-green animate-spin shrink-0" />
          <p className="text-sm">
            <span className="font-semibold">{justLinked}</span> linked. First sync running in the background — accounts and transactions will appear in a few seconds.
          </p>
        </div>
      )}

      {/* Add a new bank — full width, prominent */}
      <section className="card p-6 md:p-8 fade-in-1">
        <div className="flex flex-col md:flex-row md:items-end gap-4">
          <div className="flex-1">
            <label className="block text-[10px] uppercase tracking-widest text-mid mb-2">
              Add a new bank
            </label>
            <form
              action="/consents"
              method="post"
              encType="application/x-www-form-urlencoded"
              className="flex gap-3"
            >
              <input
                className="input flex-1"
                name="nickname"
                placeholder="Nickname (e.g. nationwide, santander, chase)"
                value={nickname}
                onChange={(e) => setNickname(e.target.value)}
                required
              />
              <button className="btn-cta" type="submit">
                Start OAuth flow <ArrowRight className="size-4" />
              </button>
            </form>
            <p className="text-xs text-mid mt-2.5">
              You'll be redirected to TrueLayer → pick your bank → log in → approve → bounce
              back here. The first hour after consent gives full transaction history; after
              that banks restrict to ~90 days, so link in succession.
            </p>
          </div>
        </div>
      </section>

      {/* Linked banks — card grid */}
      <section className="space-y-4 fade-in-2">
        <h2 className="text-sm font-semibold uppercase tracking-widest text-mid">
          {consents.data?.consents.length ?? 0} linked
        </h2>
        {!consents.data?.consents.length ? (
          <div className="card p-12 text-center">
            <Wallet className="size-8 text-mid mx-auto mb-4" />
            <p className="text-mid text-sm">No banks linked yet. Add one above.</p>
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
            {consents.data.consents.map((c) => (
              <BankCard
                key={c.id}
                consent={c}
                accounts={accountsByConsent[c.id] ?? []}
                onRename={(name) =>
                  rename.mutate({ id: c.id, nickname: name })
                }
                onRenameAccount={(id, name) =>
                  renameAccount.mutate({ id, name })
                }
                onSync={() => syncConsent.mutate(c.id)}
                onRemove={() => removeConsent.mutate(c.id)}
                syncing={syncConsent.isPending && syncConsent.variables === c.id}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function BankCard({
  consent,
  accounts,
  onRename,
  onRenameAccount,
  onSync,
  onRemove,
  syncing,
}: {
  consent: Consent;
  accounts: Account[];
  onRename: (newNickname: string) => void;
  onRenameAccount: (accountId: number, newName: string | null) => void;
  onSync: () => void;
  onRemove: () => void;
  syncing: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(consent.nickname);

  const status = consent.last_sync_status;
  const statusPill =
    status === "success" ? (
      <span className="pill-green inline-flex items-center gap-1">
        <CheckCircle2 className="size-3" /> healthy
      </span>
    ) : status === "fail" ? (
      <span className="pill-orange inline-flex items-center gap-1">
        <AlertTriangle className="size-3" /> failing
      </span>
    ) : status === "partial" ? (
      <span className="pill-orange">partial</span>
    ) : (
      <span className="pill-grey">never synced</span>
    );

  return (
    <article className="card p-6 space-y-4">
      <header className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          {editing ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                onRename(draft);
                setEditing(false);
              }}
              className="flex gap-2"
            >
              <input
                className="input flex-1"
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
              />
              <button className="btn-primary text-xs" type="submit">
                Save
              </button>
              <button
                type="button"
                className="btn-ghost text-xs"
                onClick={() => {
                  setDraft(consent.nickname);
                  setEditing(false);
                }}
              >
                Cancel
              </button>
            </form>
          ) : (
            <div className="flex items-center gap-2">
              <h3 className="text-xl font-extrabold truncate">
                {consent.nickname}
              </h3>
              <button
                className="btn-ghost p-1 -m-1"
                onClick={() => {
                  setDraft(consent.nickname);
                  setEditing(true);
                }}
                aria-label="Rename bank"
              >
                <Pencil className="size-3.5" />
              </button>
            </div>
          )}
          <p className="text-[11px] mono text-mid mt-1">
            Last sync:{" "}
            {consent.last_sync_at ? relativeTime(consent.last_sync_at) : "never"}
          </p>
        </div>
        <div className="shrink-0">{statusPill}</div>
      </header>

      {accounts.length > 0 && (
        <ul className="space-y-2.5">
          {accounts.map((a) => (
            <AccountRow
              key={a.id}
              account={a}
              onRename={(name) => onRenameAccount(a.id, name)}
            />
          ))}
        </ul>
      )}

      {consent.last_sync_error && (
        <p className="text-xs text-danger mono break-all">
          {consent.last_sync_error}
        </p>
      )}

      <footer className="flex flex-wrap gap-2 pt-1">
        <button
          type="button"
          className="btn-secondary text-xs"
          onClick={onSync}
          disabled={syncing}
        >
          <RotateCw className={`size-3.5 ${syncing ? "animate-spin" : ""}`} />{" "}
          {syncing ? "Syncing…" : "Sync now"}
        </button>
        <button
          type="button"
          className="btn-ghost text-xs text-danger"
          onClick={() => {
            if (
              confirm(
                `Remove "${consent.nickname}"? Transactions stay, but no more syncing.`
              )
            ) {
              onRemove();
            }
          }}
        >
          <Trash2 className="size-3.5" /> Remove
        </button>
      </footer>
    </article>
  );
}

function AccountRow({
  account,
  onRename,
}: {
  account: Account;
  onRename: (name: string | null) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(
    account.custom_display_name ?? account.display_name
  );
  const displayed = account.custom_display_name ?? account.display_name;
  const balance =
    account.current_balance_cents != null
      ? `${(account.current_balance_cents / 100).toLocaleString("en-GB", {
          style: "currency",
          currency: account.currency || "GBP",
        })}`
      : "—";
  const isOverdrawn =
    account.current_balance_cents != null && account.current_balance_cents < 0;

  return (
    <li className="bg-cream/40 border border-thin rounded px-3 py-2.5 flex items-center justify-between gap-3">
      <div className="min-w-0 flex-1">
        {editing ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              onRename(draft.trim() || null);
              setEditing(false);
            }}
            className="flex gap-2"
          >
            <input
              className="input flex-1 text-sm"
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder={account.display_name}
            />
            <button className="btn-primary text-xs py-1" type="submit">
              Save
            </button>
            <button
              type="button"
              className="btn-ghost text-xs py-1"
              onClick={() => {
                setDraft(account.custom_display_name ?? account.display_name);
                setEditing(false);
              }}
            >
              Cancel
            </button>
          </form>
        ) : (
          <div className="flex items-center gap-2 min-w-0">
            <span className="text-sm font-semibold truncate">{displayed}</span>
            {account.account_type && (
              <span className="pill-grey text-[10px]">
                {account.account_type.replace(/_/g, " ").toLowerCase()}
              </span>
            )}
            {account.card_network && (
              <span className="pill-grey text-[10px]">
                {account.card_network}
              </span>
            )}
            <button
              className="btn-ghost p-0.5"
              onClick={() => setEditing(true)}
              aria-label="Rename account"
            >
              <Pencil className="size-3" />
            </button>
          </div>
        )}
        <p className="text-[10px] mono text-mid mt-0.5 truncate">
          {account.iban || account.card_last4
            ? `•••• ${account.card_last4}`
            : account.account_number || account.currency}
        </p>
      </div>
      <div className="text-right shrink-0">
        <p
          className={`text-sm font-extrabold mono ${
            isOverdrawn ? "text-danger" : "text-ink"
          }`}
        >
          {balance}
        </p>
        {account.available_balance_cents != null &&
          account.available_balance_cents !== account.current_balance_cents && (
            <p className="text-[10px] mono text-mid">
              {(account.available_balance_cents / 100).toLocaleString("en-GB", {
                style: "currency",
                currency: account.currency || "GBP",
              })}{" "}
              avail
            </p>
          )}
      </div>
    </li>
  );
}
