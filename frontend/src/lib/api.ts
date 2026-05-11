const API_BASE_URL =
  process.env.NEXT_PUBLIC_API_BASE_URL ?? (typeof window === "undefined" ? "http://127.0.0.1:8000" : "");

export async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE_URL}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
    cache: "no-store",
  });
  if (!response.ok) {
    const payload = (await response.json().catch(() => null)) as { detail?: string; error?: string } | null;
    const message = payload?.detail || `Request failed with status ${response.status}`;
    throw new Error(payload?.error ? `${message} (${payload.error})` : message);
  }
  return (await response.json()) as T;
}

export const getFetcher = <T>(path: string) => apiFetch<T>(path);

export async function postAction<T>(path: string, body?: unknown): Promise<T> {
  return apiFetch<T>(path, {
    method: "POST",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}
