// Thin fetch wrapper around tally's REST API. Handles cookie session + 401 redirects.

export type ApiError = { status: number; message: string };

async function request<T>(
  path: string,
  init: RequestInit = {}
): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      ...(init.headers || {}),
    },
    credentials: "same-origin",
  });
  if (res.status === 401) {
    // Redirect to login if we're not already there.
    if (!window.location.pathname.startsWith("/login")) {
      window.location.href = "/login";
    }
    throw { status: 401, message: "unauthenticated" } satisfies ApiError;
  }
  if (!res.ok) {
    const txt = await res.text();
    throw { status: res.status, message: txt } satisfies ApiError;
  }
  // Some endpoints return empty bodies for ok responses.
  const text = await res.text();
  return (text ? JSON.parse(text) : {}) as T;
}

export const api = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "POST", body: body ? JSON.stringify(body) : undefined }),
  patch: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "PATCH", body: body ? JSON.stringify(body) : undefined }),
  delete: <T>(path: string) => request<T>(path, { method: "DELETE" }),
};
