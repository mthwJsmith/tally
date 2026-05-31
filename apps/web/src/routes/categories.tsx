import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import * as LucideIcons from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { api } from "@/lib/api";
import type { Category } from "@/types/api";

export const Route = createFileRoute("/categories")({
  component: CategoriesPage,
});

function CategoryIcon({ name }: { name: string | null }) {
  if (!name) return null;
  const Icon = (LucideIcons as unknown as Record<string, LucideIcon>)[name];
  if (!Icon) return null;
  return <Icon className="size-4 text-green" />;
}

function CategoriesPage() {
  const qc = useQueryClient();
  const q = useQuery({
    queryKey: ["categories"],
    queryFn: () => api.get<{ categories: Category[] }>("/api/categories"),
  });
  const [name, setName] = useState("");
  const [icon, setIcon] = useState("");

  const create = useMutation({
    mutationFn: () => api.post("/api/categories", { name, icon: icon || null }),
    onSuccess: () => {
      setName("");
      setIcon("");
      qc.invalidateQueries({ queryKey: ["categories"] });
    },
  });
  const del = useMutation({
    mutationFn: (id: number) => api.delete(`/api/categories/${id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["categories"] }),
  });

  return (
    <div className="p-8 md:p-12 max-w-3xl space-y-8">
      <header className="fade-in">
        <h1 className="text-4xl mb-2">
          <em>Categories</em>
        </h1>
        <p className="text-mid text-sm">
          Group transactions for budgets and reporting.
        </p>
      </header>

      <form
        className="card p-5 flex flex-wrap gap-2 items-center fade-in-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) create.mutate();
        }}
      >
        <input
          className="input flex-1 min-w-[180px]"
          placeholder="Category name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input max-w-[180px]"
          placeholder="Lucide icon (e.g. ShoppingCart)"
          value={icon}
          onChange={(e) => setIcon(e.target.value)}
        />
        <button className="btn-primary" disabled={create.isPending}>
          <Plus className="size-4" /> Add
        </button>
      </form>

      <ul className="card divide-y divide-thin fade-in-2">
        {q.data?.categories.map((c) => (
          <li
            key={c.id}
            className="flex items-center justify-between px-5 py-3.5 text-sm"
          >
            <span className="flex items-center gap-2.5">
              <CategoryIcon name={c.icon} />
              {c.name}
            </span>
            <button
              className="btn-ghost text-xs"
              onClick={() => del.mutate(c.id)}
            >
              <Trash2 className="size-3.5" /> Delete
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
