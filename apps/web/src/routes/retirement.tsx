/**
 * Retirement forecast — "I want to retire at N, am I on track?"
 *
 * The backend does all the maths (see api/retirement.rs): the LGPS defined benefit and
 * state pension arrive at ~67/68 regardless; the invested pot (AJ Bell SIPP + SCAVCs)
 * has to bridge the years in between. This page edits the assumptions and shows the
 * verdict + the monthly saving needed. Everything is in today's money.
 */
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  Tooltip,
  XAxis,
  YAxis,
  CartesianGrid,
  ReferenceLine,
} from "recharts";
import { api } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import { NotesPanel } from "@/components/NotesPanel";
import {
  chartColors,
  chartAxisProps,
  chartTooltipStyle,
  chartGradients,
} from "@/lib/chart-theme";

export const Route = createFileRoute("/retirement")({ component: RetirementPage });

type Plan = {
  birth_date: string | null;
  target_age: number;
  target_income_annual: number;
  growth_pct: number;
  salary_annual: number;
  lgps_start: string | null;
  lgps_age: number;
  state_pension_annual: number;
  state_pension_age: number;
  monthly_contribution: number;
  include_general_investments: boolean;
};

type Forecast = {
  configured: boolean;
  age_now: number | null;
  years_to_target: number | null;
  pot_now: number;
  lgps_annual_at_target: number;
  lgps_service_years_at_target: number;
  required_pot: number;
  bridge_cost: number;
  topup_cost: number;
  perpetual_cost: number;
  projected_pot: number;
  on_track: boolean;
  shortfall: number;
  required_monthly: number;
  projection: { age: number; projected: number; with_required: number }[];
};

function RetirementPage() {
  const qc = useQueryClient();
  const data = useQuery({
    queryKey: ["retirement"],
    queryFn: () => api.get<{ plan: Plan; forecast: Forecast }>("/api/retirement"),
  });

  const [draft, setDraft] = useState<Partial<Plan>>({});
  // Seed the form once the stored plan arrives.
  useEffect(() => {
    if (data.data?.plan) setDraft(data.data.plan);
  }, [data.data?.plan]);

  const save = useMutation({
    mutationFn: (patch: Partial<Plan>) =>
      api.put<{ plan: Plan; forecast: Forecast }>("/api/retirement", patch),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["retirement"] }),
  });

  const notes = useQuery({
    queryKey: ["retirement-notes"],
    queryFn: () => api.get<{ text: string }>("/api/retirement/notes"),
  });
  const saveNotes = useMutation({
    mutationFn: (text: string) => api.put("/api/retirement/notes", { text }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["retirement-notes"] }),
  });

  const f = data.data?.forecast;
  const plan = data.data?.plan;

  const num = (v: unknown) =>
    v === "" || v == null ? undefined : Number(v);

  const field = (
    label: string,
    key: keyof Plan,
    opts: { type?: "text" | "number" | "date"; placeholder?: string; step?: string } = {},
  ) => (
    <div>
      <label className="text-[10px] uppercase tracking-widest text-mid font-semibold">
        {label}
      </label>
      <input
        className="input mono"
        type={opts.type ?? "number"}
        step={opts.step}
        placeholder={opts.placeholder}
        value={(draft[key] as string | number | null) ?? ""}
        onChange={(e) =>
          setDraft((d) => ({
            ...d,
            [key]:
              (opts.type ?? "number") === "number"
                ? num(e.target.value)
                : e.target.value || null,
          }))
        }
      />
    </div>
  );

  const gbp = (v: number) => formatMoney(Math.round(v * 100));

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1280px]">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Retirement</em>
        </h1>
        <p className="text-mid text-sm">
          LGPS + state pension arrive around {plan?.lgps_age ?? 67}. Your invested pot
          (SIPP + SCAVCs) bridges the years before that. All figures in today&apos;s money.
        </p>
      </header>

      {/* Verdict */}
      {f && f.configured && (
        <section
          className={`card p-7 fade-in-1 border-l-4 ${
            f.on_track ? "border-l-green" : "border-l-danger"
          }`}
        >
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
            Retiring at {plan?.target_age} —{" "}
            {f.on_track ? "on track" : "not on track yet"}
          </p>
          <div className="grid md:grid-cols-4 gap-6">
            <div>
              <p className="text-[10px] uppercase tracking-widest text-mid mb-1">
                Pot needed at {plan?.target_age}
              </p>
              <p className="font-extrabold text-2xl mono">{gbp(f.required_pot)}</p>
            </div>
            <div>
              <p className="text-[10px] uppercase tracking-widest text-mid mb-1">
                Projected pot
              </p>
              <p
                className={`font-extrabold text-2xl mono ${
                  f.on_track ? "text-green" : "text-danger"
                }`}
              >
                {gbp(f.projected_pot)}
              </p>
            </div>
            <div>
              <p className="text-[10px] uppercase tracking-widest text-mid mb-1">
                Needed per month
              </p>
              <p className="font-extrabold text-2xl mono">
                {gbp(f.required_monthly)}
              </p>
              <p className="text-[11px] text-mid">
                vs {gbp(plan?.monthly_contribution ?? 0)} now
              </p>
            </div>
            <div>
              <p className="text-[10px] uppercase tracking-widest text-mid mb-1">
                Pot today
              </p>
              <p className="font-extrabold text-2xl mono">{gbp(f.pot_now)}</p>
              <p className="text-[11px] text-mid">
                {f.years_to_target?.toFixed(1)} years to go
              </p>
            </div>
          </div>
          <p className="text-sm text-mid mt-4">
            The {gbp(f.required_pot)} covers: {gbp(f.bridge_cost)} full income from{" "}
            {plan?.target_age} to {plan?.lgps_age}
            {f.topup_cost > 0 &&
              ` · ${gbp(f.topup_cost)} top-ups until the state pension at ${plan?.state_pension_age}`}
            {f.perpetual_cost > 0 &&
              ` · ${gbp(f.perpetual_cost)} (25×) for the gap that LGPS + state pension never cover`}
            {f.perpetual_cost === 0 &&
              ` — after ${plan?.lgps_age}, LGPS (~${gbp(f.lgps_annual_at_target)}/yr) + state pension cover the target income on their own`}
            .
          </p>
        </section>
      )}
      {f && !f.configured && (
        <section className="card p-7 fade-in-1">
          <p className="text-sm">
            Set your <strong>date of birth</strong> (and LGPS start date) below, then
            save — the forecast needs them to count the years.
          </p>
        </section>
      )}

      {/* Projection chart */}
      {f && f.configured && f.projection.length > 1 && (
        <section className="card p-7 fade-in-2">
          <div className="flex items-baseline justify-between mb-4">
            <h2 className="text-2xl">
              Pot <em>projection</em>
            </h2>
            <p className="text-xs uppercase tracking-widest text-mid">
              solid: current savings · dashed line: pot needed
            </p>
          </div>
          <div style={{ height: 300 }}>
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart
                data={f.projection}
                margin={{ top: 5, right: 5, left: 0, bottom: 0 }}
              >
                <defs>
                  <linearGradient
                    id={chartGradients.green.id}
                    x1="0"
                    y1="0"
                    x2="0"
                    y2="1"
                  >
                    <stop
                      offset="0%"
                      stopColor={chartGradients.green.from}
                      stopOpacity={0.35}
                    />
                    <stop
                      offset="100%"
                      stopColor={chartGradients.green.to}
                      stopOpacity={0}
                    />
                  </linearGradient>
                </defs>
                <CartesianGrid
                  strokeDasharray="2 3"
                  stroke={chartColors.border}
                  vertical={false}
                />
                <XAxis dataKey="age" {...chartAxisProps} minTickGap={24} />
                <YAxis
                  {...chartAxisProps}
                  width={65}
                  tickFormatter={(v: number) =>
                    v >= 1000 ? `${Math.round(v / 1000)}k` : String(v)
                  }
                />
                <Tooltip
                  formatter={(v: number, name: string) => [
                    gbp(v),
                    name === "projected"
                      ? "With current savings"
                      : "With required savings",
                  ]}
                  labelFormatter={(age) => `Age ${age}`}
                  contentStyle={chartTooltipStyle}
                />
                <Area
                  type="monotone"
                  dataKey="with_required"
                  stroke={chartColors.muted}
                  strokeDasharray="4 4"
                  strokeWidth={1.5}
                  fill="none"
                />
                <Area
                  type="monotone"
                  dataKey="projected"
                  stroke={chartColors.green}
                  strokeWidth={2.5}
                  fill={`url(#${chartGradients.green.id})`}
                />
                <ReferenceLine
                  y={f.required_pot}
                  stroke={chartColors.muted}
                  strokeDasharray="4 4"
                  label={{
                    value: `Needed ${gbp(f.required_pot)}`,
                    position: "insideTopLeft",
                    fontSize: 10,
                    fill: chartColors.muted,
                  }}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        </section>
      )}

      {/* Assumptions form */}
      <section className="card p-7 fade-in-3">
        <h2 className="text-2xl mb-4">
          Your <em>assumptions</em>
        </h2>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate(draft);
          }}
        >
          <div className="grid sm:grid-cols-2 md:grid-cols-3 gap-3">
            {field("Date of birth", "birth_date", { type: "date" })}
            {field("Target retirement age", "target_age")}
            {field("Target income (£/year, today's money)", "target_income_annual")}
            {field("Gross salary (£/year)", "salary_annual")}
            {field("LGPS membership start", "lgps_start", { type: "date" })}
            {field("LGPS pension age", "lgps_age")}
            {field("Monthly saving into SIPP/SCAVCs (£)", "monthly_contribution")}
            {field("Real growth %/year (after inflation)", "growth_pct", {
              step: "0.1",
            })}
            {field("State pension (£/year)", "state_pension_annual")}
            {field("State pension age", "state_pension_age")}
            <div className="flex items-end pb-2">
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={draft.include_general_investments ?? false}
                  onChange={(e) =>
                    setDraft((d) => ({
                      ...d,
                      include_general_investments: e.target.checked,
                    }))
                  }
                />
                Count non-pension investments too
              </label>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <button className="btn-cta" type="submit" disabled={save.isPending}>
              {save.isPending ? "Saving…" : "Save & recalculate"}
            </button>
            {save.isSuccess && (
              <span className="text-sm text-green">Saved.</span>
            )}
            {save.isError && (
              <span className="text-sm text-danger">Could not save.</span>
            )}
          </div>
        </form>
        <p className="text-[11px] text-mid mt-4">
          LGPS is modelled as salary ÷ 49 per service year, payable from LGPS pension
          age, inflation-linked (today&apos;s money throughout). Taking LGPS early
          instead would reduce it ~5% per year early — not modelled; the pot bridges to
          normal age instead. SCAVC = Shared Cost AVCs, where a participating employer
          pays part of the contribution via salary sacrifice.
        </p>
      </section>

      {/* Free-form pension notes — same editable panel as Ahead's "Moves to make";
          the MCP assistant reads this via retirement_forecast and can rewrite it. */}
      <NotesPanel
        title="Pension notes & plan"
        text={notes.data?.text ?? ""}
        onSave={(t) => saveNotes.mutate(t)}
        storageKey="retirement-notes-done"
        placeholder={"# My pension plan\n- [ ] Check whether my employer offers Shared Cost AVCs"}
        emptyHint="Nothing here yet — add your pension plan with the pencil, or ask your assistant to write it."
      />
    </div>
  );
}
