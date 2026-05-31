import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowRight } from "lucide-react";
import { api } from "@/lib/api";

export const Route = createFileRoute("/setup")({
  component: SetupPage,
});

function SetupPage() {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const navigate = useNavigate();
  const qc = useQueryClient();

  const setup = useMutation({
    mutationFn: () => api.post("/auth/setup", { username, password }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["me"] });
      navigate({ to: "/" });
    },
    onError: (e: any) => setError(e?.message || "Setup failed"),
  });

  return (
    <div className="card w-[460px] max-w-[95vw] p-12 fade-in">
      <h1 className="text-4xl mb-2">
        Welcome to <em className="text-green">Tally</em>
      </h1>
      <p className="text-mid text-sm mb-10">
        Create the admin account. Enrol two-factor in Settings after.
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          setError("");
          if (password !== confirm) {
            setError("Passwords don't match");
            return;
          }
          if (password.length < 8) {
            setError("Password needs at least 8 characters");
            return;
          }
          setup.mutate();
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
          placeholder="Password (8+ characters)"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <input
          className="input"
          type="password"
          placeholder="Confirm password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        {error && <p className="text-sm text-danger">{error}</p>}
        <button className="btn-cta w-full mt-2" disabled={setup.isPending}>
          {setup.isPending ? "Creating" : "Create admin"}
          <ArrowRight className="size-4" />
        </button>
      </form>
    </div>
  );
}
