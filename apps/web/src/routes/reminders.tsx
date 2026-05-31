import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Plus, Trash2, Check, AlarmClock, CreditCard } from "lucide-react";
import { api } from "@/lib/api";
import type { Reminder, Bill } from "@/types/api";

export const Route = createFileRoute("/reminders")({ component: RemindersPage });

function fmtDay(ts: number) {
  return new Date(ts * 1000).toLocaleDateString("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
  });
}

function statusOf(r: Reminder): { label: string; cls: string } {
  const now = Date.now() / 1000;
  if (r.completed_at && r.completed_at > 0) return { label: "done", cls: "text-green" };
  if (now >= r.due_at) return { label: "overdue", cls: "text-danger" };
  const days = Math.max(1, Math.ceil((r.due_at - now) / 86400));
  return { label: `due in ${days}d`, cls: "text-mid" };
}

function RemindersPage() {
  const qc = useQueryClient();
  const reminders = useQuery({
    queryKey: ["reminders"],
    queryFn: () => api.get<{ reminders: Reminder[] }>("/api/reminders"),
  });
  const bills = useQuery({
    queryKey: ["bills-upcoming"],
    queryFn: () => api.get<{ bills: Bill[] }>("/api/bills/upcoming?within_days=14"),
  });

  const [title, setTitle] = useState("");
  const [freq, setFreq] = useState("months");
  const [everyN, setEveryN] = useState(1);
  const [anchorDay, setAnchorDay] = useState("");
  const [due, setDue] = useState("");
  const [dayBefore, setDayBefore] = useState(true);

  const create = useMutation({
    mutationFn: () =>
      api.post("/api/reminders", {
        title: title.trim(),
        freq,
        every_n: everyN,
        anchor_day: anchorDay ? Number(anchorDay) : null,
        due_at: Math.floor(new Date(due).getTime() / 1000),
        notify_before: dayBefore ? 86400 : 0,
      }),
    onSuccess: () => {
      setTitle("");
      setDue("");
      qc.invalidateQueries({ queryKey: ["reminders"] });
    },
  });
  const tick = useMutation({
    mutationFn: (r: Reminder) =>
      api.post(`/api/reminders/${r.id}/${r.completed_at ? "untick" : "tick"}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["reminders"] }),
  });
  const del = useMutation({
    mutationFn: (id: number) => api.delete(`/api/reminders/${id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["reminders"] }),
  });

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Reminders</em>
        </h1>
        <p className="text-mid text-sm">Custom checklists plus your upcoming direct debits.</p>
      </header>

      <form
        className="card p-5 space-y-3 fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (title.trim() && due) create.mutate();
        }}
      >
        <input
          className="input"
          placeholder="e.g. Pay £50 into Help to Save"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
        <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
          <select className="input" value={freq} onChange={(e) => setFreq(e.target.value)}>
            <option value="hours">hourly</option>
            <option value="days">daily</option>
            <option value="weeks">weekly</option>
            <option value="months">monthly</option>
          </select>
          <input
            className="input"
            type="number"
            min={1}
            value={everyN}
            title="every N"
            onChange={(e) => setEveryN(Number(e.target.value))}
          />
          {freq === "months" && (
            <input
              className="input"
              type="number"
              min={1}
              max={31}
              placeholder="day e.g. 28"
              value={anchorDay}
              onChange={(e) => setAnchorDay(e.target.value)}
            />
          )}
          <input
            className="input"
            type="datetime-local"
            value={due}
            onChange={(e) => setDue(e.target.value)}
          />
        </div>
        <label className="flex items-center gap-2 text-sm text-mid">
          <input
            type="checkbox"
            checked={dayBefore}
            onChange={(e) => setDayBefore(e.target.checked)}
          />{" "}
          notify the day before
        </label>
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Add reminder
        </button>
      </form>

      <ul className="card divide-y divide-thin fade-in-2">
        {reminders.data?.reminders.map((r) => {
          const s = statusOf(r);
          return (
            <li key={`r${r.id}`} className="px-5 py-3.5 flex items-center gap-3">
              <button className="btn-ghost shrink-0" title="tick" onClick={() => tick.mutate(r)}>
                <Check className={`size-4 ${r.completed_at ? "text-green" : "text-soft"}`} />
              </button>
              <div className="flex-1">
                <p className="font-semibold text-sm flex items-center gap-2">
                  <AlarmClock className="size-3.5 text-mid" /> {r.title}
                </p>
                <p className={`text-[11px] mono ${s.cls}`}>
                  {s.label} · {fmtDay(r.due_at)}
                </p>
              </div>
              <button className="btn-ghost text-xs" onClick={() => del.mutate(r.id)}>
                <Trash2 className="size-3.5" />
              </button>
            </li>
          );
        })}
        {(bills.data?.bills ?? []).map((b) => (
          <li key={`b${b.id}`} className="px-5 py-3.5 flex items-center gap-3">
            <CreditCard className="size-4 text-mid shrink-0" />
            <div className="flex-1">
              <p className="font-semibold text-sm">{b.name}</p>
              <p className="text-[11px] mono text-mid">
                direct debit
                {b.next_expected_date ? ` · due ${fmtDay(b.next_expected_date)}` : ""}
              </p>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
