/**
 * Editable notes panel — same interaction pattern as Ahead's "Moves to make"
 * (pencil → textarea → save, "- " lines become tickable items), reused for the
 * Retirement page's pension notes. Tick state is local-only (per browser);
 * the text itself is shared with the MCP assistant, which can rewrite it.
 */
import { useState } from "react";
import { ListChecks, Pencil } from "lucide-react";

interface Props {
  title: string;
  text: string;
  onSave: (t: string) => void;
  /** localStorage key for tick persistence. */
  storageKey: string;
  placeholder?: string;
  emptyHint?: string;
}

export function NotesPanel({
  title,
  text,
  onSave,
  storageKey,
  placeholder,
  emptyHint,
}: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(text);
  const [done, setDone] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem(storageKey) || "[]"));
    } catch {
      return new Set();
    }
  });
  const toggle = (key: string) =>
    setDone((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      try {
        localStorage.setItem(storageKey, JSON.stringify([...next]));
      } catch {
        /* ignore */
      }
      return next;
    });

  const lines = text.split("\n");
  const hasItems = lines.some((l) => l.trim());

  return (
    <section className="card p-5 fade-in">
      <div className="flex items-center justify-between mb-2">
        <h2 className="text-[10px] uppercase tracking-widest text-mid font-semibold flex items-center gap-1.5">
          <ListChecks className="size-3.5" /> {title}
        </h2>
        <button
          className="btn-ghost text-xs px-1.5"
          onClick={() => {
            setDraft(text);
            setEditing((v) => !v);
          }}
          title="Edit"
        >
          <Pencil className="size-3.5" />
        </button>
      </div>
      {editing ? (
        <div className="space-y-2">
          <textarea
            className="input text-sm mono"
            rows={16}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={placeholder}
          />
          <div className="flex gap-2">
            <button
              className="btn-primary py-1 text-sm"
              onClick={() => {
                onSave(draft);
                setEditing(false);
              }}
            >
              Save
            </button>
            <button
              className="btn-ghost text-sm py-1"
              onClick={() => setEditing(false)}
            >
              Cancel
            </button>
          </div>
          <p className="text-[11px] text-mid">
            Markdown-ish: “#” lines become headings, “- ” lines become tick items.
            Your assistant can read and rewrite this anytime.
          </p>
        </div>
      ) : !hasItems ? (
        <p className="text-sm text-mid">{emptyHint ?? "Nothing here yet — add notes with the pencil."}</p>
      ) : (
        <ul className="space-y-1">
          {lines.map((line, i) => {
            const t = line.trim();
            if (!t) return null;
            if (/^-{3,}$/.test(t)) return <li key={i} className="border-t border-thin my-2" />;
            const bullet = t.match(/^[-*]\s+(?!\[)(.*)$/);
            const ticked = t.match(/^[-*]\s+\[([ xX])\]\s+(.*)$/);
            if (ticked) {
              const item = ticked[2];
              const isDone = ticked[1] !== " " || done.has(item);
              return (
                <li key={i} className="flex items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    className="mt-0.5 shrink-0"
                    checked={isDone}
                    onChange={() => toggle(item)}
                  />
                  <span className={isDone ? "line-through text-mid" : "text-ink"}>{item}</span>
                </li>
              );
            }
            if (bullet) {
              const item = bullet[1];
              const isDone = done.has(item);
              return (
                <li key={i} className="flex items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    className="mt-0.5 shrink-0"
                    checked={isDone}
                    onChange={() => toggle(item)}
                  />
                  <span className={isDone ? "line-through text-mid" : "text-ink"}>{item}</span>
                </li>
              );
            }
            const heading = t.match(/^(#{1,6})\s+(.*)$/);
            if (heading) {
              return (
                <li
                  key={i}
                  className={`${
                    heading[1].length <= 2
                      ? "text-sm font-extrabold tracking-tight pt-3"
                      : "text-[10px] uppercase tracking-widest text-mid font-semibold pt-2"
                  } first:pt-0`}
                >
                  {heading[2].replace(/\*+/g, "")}
                </li>
              );
            }
            return (
              <li key={i} className="text-sm text-mid whitespace-pre-wrap">
                {t.replace(/\*\*/g, "")}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
