import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
  CheckCircle2,
  ShieldCheck,
  Sparkles,
  KeyRound,
  AlertTriangle,
  Plug,
  Copy,
} from "lucide-react";
import { api } from "@/lib/api";
import type { MeResponse } from "@/types/api";

export const Route = createFileRoute("/settings")({ component: SettingsPage });

function SettingsPage() {
  const me = useQuery({ queryKey: ["me"], queryFn: () => api.get<MeResponse>("/auth/me") });
  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Settings</em>
        </h1>
        <p className="text-mid text-sm">
          Account, two-factor and AI categorisation.
        </p>
      </header>

      <section className="card p-6 fade-in-1">
        <h2 className="text-lg font-semibold mb-1">Account</h2>
        <p className="text-sm text-mid">
          Logged in as <span className="font-mono text-ink">{me.data?.username}</span>
        </p>
      </section>

      <TwoFactorSection enrolled={!!me.data?.totp_enrolled} />
      <AiSettingsSection />
      <McpConnectSection />
    </div>
  );
}

function CopyBox({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div>
      <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
        {label}
      </label>
      <div className="flex items-stretch gap-2 mt-1">
        <code className="mono text-xs bg-cream border border-thin rounded px-2 py-1.5 flex-1 overflow-x-auto whitespace-pre">
          {value}
        </code>
        <button type="button" className="btn-ghost shrink-0" onClick={copy}>
          <Copy className="size-3.5" /> {copied ? "Copied ✓" : "Copy"}
        </button>
      </div>
    </div>
  );
}

function McpConnectSection() {
  const origin = window.location.origin;
  const mcpUrl = `${origin}/mcp`;
  const desktopJson = JSON.stringify(
    { mcpServers: { tally: { type: "http", url: mcpUrl } } },
    null,
    2,
  );
  return (
    <section className="card p-6 space-y-4 fade-in-5">
      <h2 className="text-lg font-semibold flex items-center gap-2">
        <Plug className="size-5 text-green" /> Connect AI assistants
      </h2>
      <p className="text-sm text-mid">
        Tally is an MCP server with OAuth, so you can let Claude or ChatGPT read your
        finances (read-only: accounts, transactions, bills, net worth). No token to paste —
        you'll get a Tally login screen to authorise.
      </p>
      <div className="text-sm bg-cream/50 border border-thin rounded p-3 space-y-1">
        <p className="font-semibold">Claude.ai (web/desktop)</p>
        <ol className="list-decimal ml-5 text-mid space-y-0.5">
          <li>Settings → <b>Connectors</b> (or Customise → Connectors)</li>
          <li>Click <b>+ Add custom connector</b></li>
          <li>Name it <b>Tally</b>, paste the URL below as the Remote MCP server URL</li>
          <li>Click Add → then <b>Connect</b> → log in to Tally + approve</li>
        </ol>
        <p className="text-mid pt-1">
          ChatGPT: Settings → Connectors (Developer Mode, Plus/Pro+) → same URL.
        </p>
      </div>
      <CopyBox label="Remote MCP server URL" value={mcpUrl} />
      <CopyBox
        label="Claude Code (terminal)"
        value={`claude mcp add --transport http tally ${mcpUrl}`}
      />
      <CopyBox label="Claude Desktop (claude_desktop_config.json)" value={desktopJson} />
      <p className="text-xs text-mid">
        Claude app &amp; ChatGPT need their connector/developer mode (Pro/Plus and up). The
        OAuth login screen is your normal Tally username, password and 2FA.
      </p>
    </section>
  );
}

function TwoFactorSection({ enrolled }: { enrolled: boolean }) {
  const qc = useQueryClient();
  const [enrol, setEnrol] = useState<{ qr_png_base64: string; secret_b64: string } | null>(null);
  const [code, setCode] = useState("");
  const [codes, setCodes] = useState<string[] | null>(null);
  const [copied, setCopied] = useState(false);

  const startEnrol = useMutation({
    mutationFn: () =>
      api.post<{ qr_png_base64: string; secret_b64: string }>("/auth/2fa/enrol"),
    onSuccess: (r) => {
      setEnrol(r);
      setCode("");
    },
  });
  const confirm = useMutation({
    mutationFn: () =>
      api.post<{ ok: boolean; recovery_codes: string[] }>("/auth/2fa/confirm", {
        secret_b64: enrol!.secret_b64,
        code,
      }),
    onSuccess: async (r) => {
      setCodes(r.recovery_codes);
      setEnrol(null);
      setCode("");
      await qc.invalidateQueries({ queryKey: ["me"] });
    },
  });

  const submitCode = (e: React.FormEvent) => {
    e.preventDefault();
    if (code.length === 6 && !confirm.isPending) confirm.mutate();
  };

  const copyCodes = async () => {
    if (!codes) return;
    await navigator.clipboard.writeText(codes.join("\n"));
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const recoveryPanel = codes && (
    <div className="border border-orange-deep/30 bg-orange/5 p-4 mt-3 space-y-2.5">
      <p className="text-sm font-semibold text-orange flex items-center gap-1.5">
        <AlertTriangle className="size-4" /> Save these recovery codes — shown ONCE.
      </p>
      <ul className="font-mono text-xs grid grid-cols-2 gap-1">
        {codes.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
      <div className="flex gap-2 pt-1">
        <button type="button" className="btn-ghost" onClick={copyCodes}>
          {copied ? "Copied ✓" : "Copy to clipboard"}
        </button>
        <button type="button" className="btn-primary" onClick={() => setCodes(null)}>
          I've saved them
        </button>
      </div>
    </div>
  );

  return (
    <section className="card p-6 space-y-3 fade-in-2">
      <h2 className="text-lg font-semibold flex items-center gap-2">
        <ShieldCheck className="size-5 text-green" /> Two-factor authentication
      </h2>
      {enrolled ? (
        <p className="text-sm text-green flex items-center gap-1.5">
          <CheckCircle2 className="size-4" /> TOTP enrolled.
        </p>
      ) : !enrol ? (
        <>
          <p className="text-sm text-mid">
            Add an authenticator app (Aegis, 1Password, Authy) for an extra layer
            on top of your password.
          </p>
          <button
            className="btn-primary"
            onClick={() => startEnrol.mutate()}
            disabled={startEnrol.isPending}
          >
            {startEnrol.isPending ? "Generating QR…" : "Enrol 2FA"}
          </button>
        </>
      ) : (
        <form onSubmit={submitCode} className="space-y-3">
          <p className="text-sm text-mid">
            Scan with your authenticator app, then enter the 6-digit code below.
          </p>
          <img
            src={`data:image/png;base64,${enrol.qr_png_base64}`}
            alt="TOTP QR"
            className="bg-white p-2 border border-thin"
          />
          <input
            className="input max-w-[200px] tracking-[0.5em] text-center font-mono"
            placeholder="000000"
            maxLength={6}
            inputMode="numeric"
            autoComplete="one-time-code"
            autoFocus
            value={code}
            onChange={(e) => setCode(e.target.value.replace(/\D/g, ""))}
          />
          {confirm.isError && (
            <p className="text-sm text-danger">
              Code didn't match. Enter the latest 6-digit code from your authenticator and try again.
            </p>
          )}
          <button
            type="submit"
            className="btn-primary"
            disabled={confirm.isPending || code.length !== 6}
          >
            {confirm.isPending ? "Verifying…" : "Confirm & enable 2FA"}
          </button>
        </form>
      )}
      {recoveryPanel}
    </section>
  );
}

function AiSettingsSection() {
  const qc = useQueryClient();
  const settings = useQuery({
    queryKey: ["ai-settings"],
    queryFn: () =>
      api.get<{
        openrouter_configured: boolean;
        openrouter_model: string;
        auto_categorise: boolean;
      }>("/api/ai/settings"),
  });
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("");

  const autoMut = useMutation({
    mutationFn: (val: boolean) =>
      api.patch("/api/ai/settings", { auto_categorise: val }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["ai-settings"] }),
  });

  const save = useMutation({
    mutationFn: () =>
      api.patch("/api/ai/settings", {
        openrouter_api_key: apiKey || undefined,
        openrouter_model: model || undefined,
      }).catch(() =>
        api.post("/api/ai/settings", {
          openrouter_api_key: apiKey || undefined,
          openrouter_model: model || undefined,
        })
      ),
    onSuccess: async () => {
      setApiKey("");
      await qc.invalidateQueries({ queryKey: ["ai-settings"] });
    },
  });

  const clear = useMutation({
    mutationFn: () => api.post("/api/ai/settings", { clear_key: true }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["ai-settings"] }),
  });

  return (
    <section className="card p-6 space-y-3 fade-in-3">
      <h2 className="text-lg font-semibold flex items-center gap-2">
        <Sparkles className="size-5 text-orange" /> AI categorisation
      </h2>
      <p className="text-sm text-mid">
        Plug in an OpenRouter API key and Tally will suggest categories for
        uncategorised transactions. The free models on OpenRouter (Llama,
        Mistral, Gemini Flash) are plenty for this — set rate limits and let it
        chew through your backlog.
      </p>
      <p className="text-sm">
        Status:{" "}
        {settings.data?.openrouter_configured ? (
          <span className="pill-green">Configured</span>
        ) : (
          <span className="pill-grey">Not configured</span>
        )}
      </p>
      <div className="space-y-2 max-w-md">
        <div>
          <label className="text-xs font-semibold text-mid uppercase tracking-wider flex items-center gap-1">
            <KeyRound className="size-3" /> OpenRouter API key
          </label>
          <input
            className="input mt-1"
            type="password"
            placeholder={settings.data?.openrouter_configured ? "(set — paste new to replace)" : "sk-or-v1-..."}
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
          />
        </div>
        <div>
          <label className="text-xs font-semibold text-mid uppercase tracking-wider">
            Model
          </label>
          <input
            className="input mt-1 mono"
            placeholder={settings.data?.openrouter_model ?? ""}
            value={model}
            onChange={(e) => setModel(e.target.value)}
          />
          <p className="text-xs text-mid mt-1">
            Default:{" "}
            <span className="mono">
              meta-llama/llama-3.1-8b-instruct:free
            </span>
            . Try{" "}
            <span className="mono">google/gemini-flash-1.5:free</span> if Llama
            is slow.
          </p>
        </div>
      </div>
      <label className="flex items-center gap-2 text-sm cursor-pointer select-none">
        <input
          type="checkbox"
          className="size-4"
          checked={!!settings.data?.auto_categorise}
          disabled={autoMut.isPending}
          onChange={(e) => autoMut.mutate(e.target.checked)}
        />
        <span>
          Auto-categorise new transactions after each sync
          <span className="text-mid">
            {" "}
            — runs the model on up to 25 uncategorised transactions per sync
          </span>
        </span>
      </label>
      <div className="flex gap-2">
        <button
          className="btn-primary"
          onClick={() => save.mutate()}
          disabled={save.isPending}
        >
          Save
        </button>
        {settings.data?.openrouter_configured && (
          <button
            className="btn-ghost"
            onClick={() => clear.mutate()}
            disabled={clear.isPending}
          >
            Clear API key
          </button>
        )}
      </div>
    </section>
  );
}
