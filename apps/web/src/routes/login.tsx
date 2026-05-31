import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Lock } from "lucide-react";
import { api } from "@/lib/api";

export const Route = createFileRoute("/login")({
  component: LoginPage,
});

function LoginPage() {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [stage, setStage] = useState<"creds" | "totp">("creds");
  const [error, setError] = useState<string>("");
  const navigate = useNavigate();
  const qc = useQueryClient();

  const login = useMutation({
    mutationFn: (vars: { username: string; password: string }) =>
      api.post<{ ok: boolean; requires_2fa: boolean }>("/auth/login", vars),
    onSuccess: async (r) => {
      if (r.requires_2fa) setStage("totp");
      else {
        await qc.invalidateQueries({ queryKey: ["me"] });
        navigate({ to: "/" });
      }
    },
    onError: (e: any) => setError(e?.message || "Login failed"),
  });

  const verify = useMutation({
    mutationFn: () => api.post("/auth/verify-2fa", { code }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["me"] });
      navigate({ to: "/" });
    },
    onError: (e: any) => setError(e?.message || "Invalid code"),
  });

  return (
    <div className="card w-[460px] max-w-[95vw] p-12 fade-in">
      <h1 className="text-4xl mb-2">
        Tally
      </h1>
      <p className="text-mid text-sm mb-10">
        Sign in to your <em className="text-green">finances</em>.
      </p>

      {stage === "creds" ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setError("");
            login.mutate({ username, password });
          }}
          className="space-y-3"
        >
          <input
            className="input"
            placeholder="Username"
            value={username}
            autoFocus
            onChange={(e) => setUsername(e.target.value)}
          />
          <input
            className="input"
            type="password"
            placeholder="Password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          {error && <p className="text-sm text-danger">{error}</p>}
          <button className="btn-cta w-full mt-2" disabled={login.isPending}>
            {login.isPending ? "Signing in" : "Sign in"}
            <ArrowRight className="size-4" />
          </button>
        </form>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setError("");
            verify.mutate();
          }}
          className="space-y-3"
        >
          <div className="flex items-center gap-2 text-mid text-sm">
            <Lock className="size-4" />
            Enter your 6-digit authenticator code.
          </div>
          <input
            className="input text-center text-3xl tracking-[0.5em] font-mono"
            placeholder="000000"
            inputMode="numeric"
            pattern="[0-9]*"
            maxLength={6}
            value={code}
            autoFocus
            onChange={(e) => setCode(e.target.value)}
          />
          {error && <p className="text-sm text-danger">{error}</p>}
          <button className="btn-cta w-full mt-2" disabled={verify.isPending}>
            {verify.isPending ? "Verifying" : "Verify"}
            <ArrowRight className="size-4" />
          </button>
        </form>
      )}
    </div>
  );
}
